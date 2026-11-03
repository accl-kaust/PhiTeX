//! Nodes placed after the same node by different placers, ranked
//! (`Ctx::rank_placement`): they stand in the order of their ranks,
//! whichever placer commits first. Unranked, the one placed last stood
//! first: a page's finisher placed by a loop rest from a stale start stood
//! before the finisher of an earlier page after the same piece, which
//! read its `\c@page` and marks (Grail's \cite edit at -j8, mc 79257).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, NodeId, OpId, Operation, Stop, ValueId};

/// `[name, x]`: defines `name` := x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(phitex_defuse::Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[key]`: reads `m`, defines `m` := key; outputs what it read.
struct Seen {
    runs: AtomicU64,
}

impl Operation for Seen {
    fn name(&self) -> &'static str {
        "seen"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let x = cx.read(v.field("m"))?;
        cx.define(v.field("m"), cx.args()[0]);
        Ok(x.unwrap_or(ValueId::UNIT))
    }
}

/// `[after_raw, key, rank]`: places `Seen [key]` after node `after_raw`,
/// ranked `rank`.
struct Placer {
    seen: OpId,
}

impl Operation for Placer {
    fn name(&self) -> &'static str {
        "placer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let after = NodeId::from_raw(v.as_int(a[0]).unwrap_or(0).unsigned_abs());
        let rank = v.as_int(a[2]).unwrap_or(0).unsigned_abs();
        let i = cx.follow_after(after, self.seen, vec![a[1]]);
        cx.rank_placement(i, rank);
        Ok(ValueId::UNIT)
    }
}

/// `[]`: reads `m`.
struct Reader;

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        Ok(cx.read(v.field("m"))?.unwrap_or(ValueId::UNIT))
    }
}

/// Line: A, placers P1 (key 1) and P2 (key 2) each placing a `Seen` after
/// A, a reader. P1 ranks its node `r1`, P2 `r2`.
fn line(threads: usize, r1: i64, r2: i64) {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let seen_op = Arc::new(Seen {
        runs: AtomicU64::new(0),
    });
    let seen = e.register(seen_op.clone());
    let placer = e.register(Arc::new(Placer { seen }));
    let reader = e.register(Arc::new(Reader));
    let u = e.new_unit();
    let a = e.push(u, def, vec![v.field("a").0, v.int(0)]);
    let at = v.int(i64::try_from(a.raw()).unwrap());
    let p1 = e.push(u, placer, vec![at, v.int(1), v.int(r1)]);
    let p2 = e.push(u, placer, vec![at, v.int(2), v.int(r2)]);
    let r = e.push(u, reader, vec![]);
    let rep = e.evaluate(threads).unwrap();
    let m1 = e.placed_by(p1)[0];
    let m2 = e.placed_by(p2)[0];
    // (the lower rank first, the other reads its definition)
    let (lo, hi, lo_key, hi_key) = if r1 < r2 { (m1, m2, 1, 2) } else { (m2, m1, 2, 1) };
    assert!(e.before(lo, hi), "the lower rank stands first: {rep:?}");
    assert_eq!(e.output(hi).and_then(|x| v.as_int(x)), Some(lo_key), "{rep:?}");
    assert_eq!(e.output(lo), Some(ValueId::UNIT), "{rep:?}");
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(hi_key), "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());

    // P1's rank moved to P2's other side: its node moves there, kept
    let r1b = if r1 < r2 { r2 + 1 } else { r2 - 1 };
    e.replace(p1, placer, vec![at, v.int(1), v.int(r1b)]);
    let rep = e.evaluate(threads).unwrap();
    assert_eq!(e.placed_by(p1), vec![m1], "the same node, relocated: {rep:?}");
    let (lo, hi, lo_key, hi_key) = if r1b < r2 { (m1, m2, 1, 2) } else { (m2, m1, 2, 1) };
    assert!(e.before(lo, hi), "P1's node on P2's other side now: {rep:?}");
    assert_eq!(e.output(hi).and_then(|x| v.as_int(x)), Some(lo_key), "{rep:?}");
    assert_eq!(e.output(lo), Some(ValueId::UNIT), "{rep:?}");
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(hi_key), "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// P1 (the earlier placer) ranks lower: unranked, P2's node, placed last
/// at -j1, stood first and P1's read P2's definition.
#[test]
fn nodes_placed_after_one_node_stand_in_rank_order() {
    line(1, 1, 2);
    line(4, 1, 2);
}

/// P1 ranks higher: its node goes after P2's though P2 commits after it.
#[test]
fn a_higher_rank_placed_first_stays_after_a_lower_one_placed_later() {
    line(1, 3, 2);
    line(4, 3, 2);
}
