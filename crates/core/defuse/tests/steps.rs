//! What a chain of steps placing after earlier nodes needs: a run that
//! meets a hole publishes nothing new (`Ctx::unchanged`), and a node
//! placed after a node before its placer stands while it is after that
//! node.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, OpId, Operation, Report, Stop, ValueId};

/// `[s, x, ms]`: works `ms` milliseconds, contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[2]).unwrap_or(0).unsigned_abs();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        true
    }
}

/// `[x]`: defines "x" = x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.args()[0];
        cx.define(cx.values().field("x"), x);
        Ok(x)
    }
}

/// `[x]`: defines "t" = x.
struct DefT;

impl Operation for DefT {
    fn name(&self) -> &'static str {
        "def_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.args()[0];
        cx.define(cx.values().field("t"), x);
        Ok(x)
    }
}

/// `[s, keep]`: reads "t"; the first contribution to s, placed as a `Def` of it;
/// where a node not in yet stops the search, unchanged when `keep`.
struct Scan(OpId);

impl Operation for Scan {
    fn name(&self) -> &'static str {
        "scan"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let s = Field(cx.args()[0]);
        cx.read(cx.values().field("t"))?;
        match cx.next_contribution(s, None)? {
            Some((_, x)) => {
                cx.follow(self.0, vec![x]);
                Ok(x)
            }
            None => {
                if cx.stopped_at().is_some() && cx.values().as_int(cx.args()[1]) == Some(1) {
                    cx.unchanged();
                }
                Ok(ValueId::UNIT)
            }
        }
    }
}

/// `[]`: reads "x"; counts its runs.
struct Reader(AtomicU64);

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(cx.read(cx.values().field("x"))?.unwrap_or(ValueId::UNIT))
    }
}

/// The reader's runs after a slow contributor is put before the scan's
/// first one, with the scan keeping its product at the hole or not.
fn reruns(keep: bool) -> (u64, Report) {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let def = e.register(Arc::new(Def));
    let def_t = e.register(Arc::new(DefT));
    let scan = e.register(Arc::new(Scan(def)));
    let reader = Arc::new(Reader(AtomicU64::new(0)));
    let rd = e.register(reader.clone());
    let s = v.field("s");
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let c = e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    e.push(u, scan, vec![s.0, v.int(i64::from(keep))]);
    e.push(u, rd, vec![]);
    e.evaluate(4).unwrap();
    let before = reader.0.load(Ordering::Relaxed);
    // (a contributor that takes a while, before the first, and the scan
    // asked again meanwhile: it meets the new one before it is in)
    e.insert_before(c, contrib, vec![s.0, v.int(2), v.int(60)]);
    e.replace(t, def_t, vec![v.int(1)]);
    let r = e.evaluate(4).unwrap();
    assert!(e.check_invariants().is_empty());
    (reader.0.load(Ordering::Relaxed) - before, r)
}

#[test]
fn a_run_at_a_hole_leaves_what_it_placed_until_the_hole_fills() {
    let (kept, r) = reruns(true);
    assert_eq!(kept, 1, "the reader runs once, on the new value: {r:?}");
}

/// `[at, t]`: reads "t"; places a `Def` of 7 after node `at` and its
/// children, then a `Def` of 8 after that one.
struct PlaceBack(OpId);

impl Operation for PlaceBack {
    fn name(&self) -> &'static str {
        "place_back"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.read(v.field("t"))?;
        let at = phitex_defuse::NodeId::from_raw(v.as_int(cx.args()[0]).unwrap_or(0).unsigned_abs());
        cx.follow_after(at, self.0, vec![v.int(7)]);
        cx.follow(self.0, vec![v.int(8)]);
        Ok(ValueId::UNIT)
    }
}

#[test]
fn placements_after_a_node_before_the_placer_stand_when_the_placer_runs_again() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let def_t = e.register(Arc::new(DefT));
    let back = e.register(Arc::new(PlaceBack(def)));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let a = e.push(u, def, vec![v.int(1)]);
    let p = e.push(u, back, vec![v.int(i64::try_from(a.raw()).unwrap()), v.int(0)]);
    e.evaluate(2).unwrap();
    let kids = e.children(p);
    assert_eq!(kids.len(), 2);
    // (both stand before the placer, after `a`)
    let order = e.order(u);
    let at = |n| order.iter().position(|&x| x == n).unwrap();
    assert!(
        at(a) < at(kids[0]) && at(kids[0]) < at(kids[1]) && at(kids[1]) < at(p),
        "{order:?}"
    );
    e.replace(t, def_t, vec![v.int(1)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.children(p), kids, "the placer ran again and placed the same: {r:?}");
    assert_eq!(Report::by_op(&r.runs_by_op, "def"), 0, "nothing made anew runs: {r:?}");
    assert!(e.check_invariants().is_empty());
}

/// `[a, b]`: reads "t"; places a `Def` of 7 after node `a` (`t` = 0) or
/// `b` (else) and its children, then a `Def` of 8 after that one.
struct PlaceAt(OpId);

impl Operation for PlaceAt {
    fn name(&self) -> &'static str {
        "place_at"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = v.as_int(cx.read(v.field("t"))?.unwrap_or(v.int(0))).unwrap_or(0);
        let which = cx.args()[usize::from(t != 0)];
        let at = phitex_defuse::NodeId::from_raw(v.as_int(which).unwrap_or(0).unsigned_abs());
        cx.follow_after(at, self.0, vec![v.int(7)]);
        cx.follow(self.0, vec![v.int(8)]);
        Ok(ValueId::UNIT)
    }
}

/// A node placed after another placement of the same run stands right
/// after it: when that placement moves back (a page step's finisher,
/// placed after the piece its page fired in, moves to an earlier piece
/// when the loop before it changes), the one following it goes with it,
/// never left where it was with other nodes between (a page step left
/// behind put its own finisher after the pieces that read it, and the
/// loop never settled).
#[test]
fn a_placement_following_a_moved_one_moves_with_it() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let def_t = e.register(Arc::new(DefT));
    let at = e.register(Arc::new(PlaceAt(def)));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(1)]);
    let a = e.push(u, def, vec![v.int(1)]);
    let x = e.push(u, def, vec![v.int(2)]);
    let b = e.push(u, def, vec![v.int(3)]);
    let raw = |n: phitex_defuse::NodeId| v.int(i64::try_from(n.raw()).unwrap());
    let p = e.push(u, at, vec![raw(a), raw(b)]);
    e.evaluate(2).unwrap();
    let kids = e.children(p);
    assert_eq!(kids.len(), 2);
    let order = e.order(u);
    let pos = |o: &Vec<phitex_defuse::NodeId>, n| o.iter().position(|&m| m == n).unwrap();
    assert!(
        pos(&order, b) + 1 == pos(&order, kids[0])
            && pos(&order, kids[0]) + 1 == pos(&order, kids[1])
            && pos(&order, kids[1]) < pos(&order, p),
        "{order:?}"
    );
    // (the first placement moves back after `a`: the second goes with it)
    e.replace(t, def_t, vec![v.int(0)]);
    let r = e.evaluate(2).unwrap();
    let kids = e.children(p);
    assert_eq!(kids.len(), 2, "{r:?}");
    let order = e.order(u);
    assert!(
        pos(&order, a) + 1 == pos(&order, kids[0])
            && pos(&order, kids[0]) + 1 == pos(&order, kids[1])
            && pos(&order, kids[1]) < pos(&order, x),
        "the second placement stands right after the first, after a: {order:?}"
    );
    assert!(e.check_invariants().is_empty());
}

/// `[a]`: reads "t"; places a `Def` of 7 after node `a` and its
/// children, then a `Def` of 8 after that one (a page step's finisher
/// and the step after it).
struct PlaceAfter(OpId);

impl Operation for PlaceAfter {
    fn name(&self) -> &'static str {
        "place_after"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.read(v.field("t"))?;
        let at = phitex_defuse::NodeId::from_raw(v.as_int(cx.args()[0]).unwrap_or(0).unsigned_abs());
        cx.follow_after(at, self.0, vec![v.int(7)]);
        cx.follow(self.0, vec![v.int(8)]);
        Ok(ValueId::UNIT)
    }
}

/// `[]`: reads "t"; from its second run on, places a `Def` of 9 as its
/// continuation (a piece's rest after a page fired in it).
struct Continues(OpId);

impl Operation for Continues {
    fn name(&self) -> &'static str {
        "continues"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = v.as_int(cx.read(v.field("t"))?.unwrap_or(v.int(0))).unwrap_or(0);
        if t != 0 {
            cx.follow(self.0, vec![v.int(9)]);
        }
        Ok(ValueId::UNIT)
    }
}

/// A node's continuation (`Ctx::follow`) goes after what others placed
/// after the node and its children, and after what follows those: a
/// piece's rest after the finisher of the page that fired in it and the
/// step after that finisher, whichever came first. Placed before them,
/// the step would stand after the rest of the text and put its own
/// finisher there.
#[test]
fn a_continuation_goes_after_what_others_placed_after_the_node() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let def_t = e.register(Arc::new(DefT));
    let after = e.register(Arc::new(PlaceAfter(def)));
    let cont = e.register(Arc::new(Continues(def)));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let a = e.push(u, cont, vec![]);
    let x = e.push(u, def, vec![v.int(2)]);
    let p = e.push(u, after, vec![v.int(i64::try_from(a.raw()).unwrap())]);
    e.evaluate(2).unwrap();
    let kids = e.children(p);
    assert_eq!(kids.len(), 2);
    let order = e.order(u);
    let pos = |o: &Vec<phitex_defuse::NodeId>, n| o.iter().position(|&m| m == n).unwrap();
    assert!(
        pos(&order, a) + 1 == pos(&order, kids[0]) && pos(&order, kids[0]) + 1 == pos(&order, kids[1]),
        "{order:?}"
    );
    // (`a` now places its continuation: after the two, before `x`)
    e.replace(t, def_t, vec![v.int(1)]);
    let r = e.evaluate(2).unwrap();
    let c = e.children(a);
    assert_eq!(c.len(), 1, "{r:?}");
    let order = e.order(u);
    assert!(
        pos(&order, kids[1]) + 1 == pos(&order, c[0]) && pos(&order, c[0]) + 1 == pos(&order, x),
        "the continuation stands after the finisher and the step, before x: {order:?}"
    );
    assert_eq!(e.children(p), kids, "the step's placements stand: {r:?}");
    assert!(e.check_invariants().is_empty());
}

/// `[ms]`: works `ms` milliseconds, then defines "c" as a relation +1 on
/// the value reaching it; predicts that relation (`Operation::predicted_rels`).
struct Counter;

impl Operation for Counter {
    fn name(&self) -> &'static str {
        "counter"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[0]).unwrap_or(0).unsigned_abs();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        let c = cx.values().field("c");
        cx.define_rel(c, 1);
        Ok(ValueId::UNIT)
    }

    fn may_define(&self, _: &[ValueId], v: &phitex_defuse::Values) -> phitex_defuse::MayDefine {
        phitex_defuse::MayDefine::Names(vec![v.field("c")])
    }

    fn predicted_rels(&self, _: &[ValueId], v: &phitex_defuse::Values) -> Option<Vec<(Field, i64)>> {
        Some(vec![(v.field("c"), 1)])
    }
}

/// `[]`: reads "c"; counts its runs; its output the value.
struct ReadC(AtomicU64);

impl Operation for ReadC {
    fn name(&self) -> &'static str {
        "read_c"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        Ok(cx.read(v.field("c"))?.unwrap_or(v.int(0)))
    }
}

/// A relation an operation predicts from its arguments is filed when the
/// node is made, as a learned shape is: a reader after two slow counters
/// reads 2 before they run, and their runs, defining what was predicted,
/// re-ask nobody (a `\stepcounter` in a piece's text, predicted by the
/// hint pass: the readers after it right the first time).
#[test]
fn a_predicted_relation_stands_until_the_run_confirms_it() {
    let e = Engine::new();
    let v = e.values();
    let counter = e.register(Arc::new(Counter));
    let reader = Arc::new(ReadC(AtomicU64::new(0)));
    let rd = e.register(reader.clone());
    let u = e.new_unit();
    e.push(u, counter, vec![v.int(40)]);
    e.push(u, counter, vec![v.int(40)]);
    let r = e.push(u, rd, vec![]);
    let rep = e.evaluate(4).unwrap();
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(2), "{rep:?}");
    assert_eq!(
        reader.0.load(Ordering::Relaxed),
        1,
        "the reader ran once, on the predicted value: {rep:?}"
    );
    assert!(e.check_invariants().is_empty());
}
