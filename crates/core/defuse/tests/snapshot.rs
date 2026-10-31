//! Family snapshots (docs/spec-state-model.md §2.1, §9): a client loads a
//! whole table through one unrecorded `Ctx::snapshot` and records only
//! the items it used with `Ctx::read_item`, so a definition of one item
//! re-asks only the readers of that item.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId, Values};

/// `[name, value]`: defines field `name` := value.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[family, used...]`: loads the family's snapshot, uses the named items
/// (records a read of each with the value the snapshot held), outputs
/// the sum of the used items' values. Counts its runs.
struct User {
    runs: AtomicU64,
}

impl Operation for User {
    fn name(&self) -> &'static str {
        "user"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let fam = Field(cx.args()[0]);
        let snap = cx.snapshot(fam)?;
        // (the snapshot: a list of [field, value] pairs in field order)
        let pairs = v.with_list(snap, <[ValueId]>::to_vec).unwrap_or_default();
        let mut sum = 0i64;
        let used: Vec<ValueId> = cx.args()[1..].to_vec();
        for u in used {
            let f = Field(u);
            let seen = pairs.iter().find_map(|&p| {
                let xs = v.with_list(p, <[ValueId]>::to_vec).unwrap_or_default();
                (xs.first() == Some(&f.0)).then(|| xs.get(1).copied()).flatten()
            });
            cx.read_item(f, seen)?;
            sum += seen.and_then(|x| v.as_int(x)).unwrap_or(0);
        }
        Ok(v.int(sum))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// Three items of family `cat:` defined before a user that uses two of
/// them. Changing the third re-asks nobody; changing a used one re-asks
/// the user once, and its new sum is right.
#[test]
fn a_snapshot_is_no_read_and_a_used_item_is() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let us = Arc::new(User {
        runs: AtomicU64::new(0),
    });
    let user = e.register(us.clone());
    let fam = v.field("cat:");
    let (a, b, c) = (v.field("cat:65"), v.field("cat:66"), v.field("cat:67"));
    let u = e.new_unit();
    let na = e.push(u, def, vec![a.0, v.int(11)]);
    e.push(u, def, vec![b.0, v.int(12)]);
    let nc = e.push(u, def, vec![c.0, v.int(13)]);
    let x = e.push(u, user, vec![fam.0, a.0, b.0]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(e.output(x).and_then(|y| v.as_int(y)), Some(23), "{r:?}");
    let runs0 = us.runs.load(Ordering::SeqCst);
    // the unused item changes: no re-ask
    e.replace(nc, def, vec![c.0, v.int(99)]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(
        us.runs.load(Ordering::SeqCst),
        runs0,
        "a change of an item the user did not use re-asks it not: {r:?}"
    );
    // a used item changes: one re-ask, the new sum
    e.replace(na, def, vec![a.0, v.int(21)]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(us.runs.load(Ordering::SeqCst), runs0 + 1, "{r:?}");
    assert_eq!(e.output(x).and_then(|y| v.as_int(y)), Some(33), "{r:?}");
    assert!(e.check_invariants().is_empty());
}

/// A snapshot's content is the same value id wherever the same
/// definitions reach (cached by content), and reflects only definitions
/// before the reader's position.
#[test]
fn a_snapshot_folds_what_reaches_the_reader_and_is_shared_by_content() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let us = Arc::new(User {
        runs: AtomicU64::new(0),
    });
    let user = e.register(us.clone());
    let fam = v.field("cat:");
    let (a, b) = (v.field("cat:65"), v.field("cat:66"));
    let u = e.new_unit();
    e.push(u, def, vec![a.0, v.int(1)]);
    let x1 = e.push(u, user, vec![fam.0, a.0, b.0]);
    let x2 = e.push(u, user, vec![fam.0, a.0, b.0]);
    e.push(u, def, vec![b.0, v.int(2)]);
    let x3 = e.push(u, user, vec![fam.0, a.0, b.0]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(e.output(x1).and_then(|y| v.as_int(y)), Some(1), "{r:?}");
    assert_eq!(e.output(x2).and_then(|y| v.as_int(y)), Some(1), "{r:?}");
    assert_eq!(
        e.output(x3).and_then(|y| v.as_int(y)),
        Some(3),
        "b is defined only after x1 and x2: {r:?}"
    );
    assert!(e.check_invariants().is_empty());
}
