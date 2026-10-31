//! A client's edit of a run's placements (`Engine::insert_placed`,
//! `Engine::remove_placed`): a node it places beside a placement is the
//! placer's, kept by the placer's next run, never made a second time; one
//! it removes is not made again.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, OpId, Operation, Report, Stop, ValueId};

/// `[x]`: outputs x. Counts its runs.
struct Leaf(AtomicU64);

impl Operation for Leaf {
    fn name(&self) -> &'static str {
        "leaf"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(cx.args()[0])
    }
}

/// `[x...]`: a `Leaf [x]` as its continuation for each, in order (a
/// reader of a file placing its pieces).
struct Placer(OpId);

impl Operation for Placer {
    fn name(&self) -> &'static str {
        "placer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        for x in cx.args().to_vec() {
            cx.follow(self.0, vec![x]);
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_node_a_client_places_beside_a_placement_is_its_placer_s() {
    let e = Engine::new();
    let v = e.values();
    let leaf = Arc::new(Leaf(AtomicU64::new(0)));
    let lo = e.register(leaf.clone());
    let po = e.register(Arc::new(Placer(lo)));
    let u = e.new_unit();
    let p = e.push(u, po, vec![v.int(1), v.int(3)]);
    let after = e.push(u, lo, vec![v.int(9)]);
    e.evaluate(2).unwrap();
    let kids = e.children(p);
    assert_eq!(kids.len(), 2);
    // the client inserts the piece for 2 after the one for 1, then the
    // placer's text says so and it runs again
    let n = e.insert_placed(kids[0], true, lo, vec![v.int(2)]);
    assert_eq!(
        e.children(p),
        vec![kids[0], n, kids[1]],
        "listed among its placer's placements"
    );
    e.replace(p, po, vec![v.int(1), v.int(2), v.int(3)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(
        e.children(p),
        vec![kids[0], n, kids[1]],
        "the placer's run keeps it: {r:?}"
    );
    assert_eq!(Report::by_op(&r.placed_by_op, "leaf"), 0, "no second node: {r:?}");
    let order = e.order(u);
    let at = |x| order.iter().position(|&m| m == x).unwrap();
    assert!(
        at(kids[0]) < at(n) && at(n) < at(kids[1]) && at(kids[1]) < at(after),
        "{order:?}"
    );
    assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(2));
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    // one before the first, as the client does where none precedes
    let m = e.insert_placed(kids[0], false, lo, vec![v.int(0)]);
    e.replace(p, po, vec![v.int(0), v.int(1), v.int(2), v.int(3)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.children(p), vec![m, kids[0], n, kids[1]], "{r:?}");
    assert_eq!(Report::by_op(&r.placed_by_op, "leaf"), 0, "{r:?}");
    // and removed: gone from its placer's placements, not made again
    e.remove_placed(n);
    e.replace(p, po, vec![v.int(0), v.int(1), v.int(3)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.children(p), vec![m, kids[0], kids[1]], "{r:?}");
    assert_eq!(Report::by_op(&r.placed_by_op, "leaf"), 0, "{r:?}");
    assert_eq!(e.output(n), None, "removed");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
