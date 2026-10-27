//! The second review of the value store's collection: values the engine
//! or its caller holds outside the records must survive it.

use std::sync::Arc;

use phitex_defuse::{Ctx, EditSource, Engine, Limits, NodeId, Operation, Stop, ValueId, Values};

fn made(v: &Values, k: i64, salt: i64) -> ValueId {
    v.list((0..k).map(|i| v.int(salt * 1000 + i)).collect())
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

/// `[salt]`: reads "t"; parks a list made of its salt and t the first
/// time; resumed, outputs the length of the list its state is.
struct Parks;

impl Operation for Parks {
    fn name(&self) -> &'static str {
        "parks"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = v.as_int(cx.read(v.field("t"))?.unwrap_or(v.int(0))).unwrap_or(0);
        if let Some(st) = cx.resumed() {
            let n = v.with_list(st, <[ValueId]>::len).map_or(-1, |n| i64::try_from(n).unwrap());
            cx.park(st);
            return Ok(v.int(n));
        }
        let salt = v.as_int(cx.args()[0]).unwrap_or(0);
        cx.park(made(v, 3, salt * 100 + t));
        Ok(v.int(0))
    }
}

/// `[k]`: makes a big list of its own and defines "junk" with it, to
/// fill the store between collections.
struct Junk;

impl Operation for Junk {
    fn name(&self) -> &'static str {
        "junk"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let k = v.as_int(cx.args()[0]).unwrap_or(0);
        cx.define(v.field("junk"), made(v, 50, 7000 + k));
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_parked_state_survives_a_collection() {
    // A run parks a list as its state; nothing else holds it. A
    // collection between its runs freed it (the record's state was no
    // root), and the run resumed from an empty slot.
    let e = Engine::new();
    let v = e.values();
    let def_t = e.register(Arc::new(DefT));
    let parks = e.register(Arc::new(Parks));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let p = e.push(u, parks, vec![v.int(1)]);
    e.evaluate(1).unwrap();
    e.collect();
    e.replace(t, def_t, vec![v.int(1)]);
    e.evaluate(1).unwrap();
    assert_eq!(e.output(p).and_then(|x| v.as_int(x)), Some(3), "the state resumed is the list parked");
}

/// `[salt]`: reads "t"; outputs a list made of its salt and t (a page).
struct Page;

impl Operation for Page {
    fn name(&self) -> &'static str {
        "page"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = v.as_int(cx.read(v.field("t"))?.unwrap_or(v.int(0))).unwrap_or(0);
        let salt = v.as_int(cx.args()[0]).unwrap_or(0);
        Ok(made(v, 4, salt * 100 + t))
    }
}

#[test]
fn what_a_marked_node_settled_with_survives_a_collection() {
    // A marked node settles twice in an evaluation (its input moved); the
    // client takes what settled after it. The first output, superseded on
    // the node, was no root: a collection in between freed it, and the
    // report named an empty slot.
    let e = Engine::new();
    let v = e.values();
    let def_t = e.register(Arc::new(DefT));
    let page = e.register(Arc::new(Page));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let p = e.push(u, page, vec![v.int(2)]);
    e.evaluate(1).unwrap();
    e.mark(p);
    e.replace(t, def_t, vec![v.int(1)]);
    e.evaluate(1).unwrap();
    e.replace(t, def_t, vec![v.int(2)]);
    e.evaluate(1).unwrap();
    e.collect();
    let items = e.take_settled();
    assert_eq!(items.len(), 2);
    for s in &items {
        assert_eq!(v.with_list(s.output, <[ValueId]>::len), Some(4), "{items:?}");
    }
}

/// Holds a list for an edit it makes at the second time it is asked (a
/// client's text interned before it is applied).
struct LaterEdit {
    held: ValueId,
    k: u32,
    node: NodeId,
    junk: phitex_defuse::OpId,
    len: phitex_defuse::OpId,
}

impl EditSource for LaterEdit {
    fn absorb(&mut self, eng: &Engine) -> Option<u64> {
        self.k += 1;
        match self.k {
            1 => {
                // (an edit that queues work: junk that fills the store)
                for i in 0..50 {
                    eng.insert_before(self.node, self.junk, vec![eng.values().int(i)]);
                }
                Some(1)
            }
            2 => {
                eng.replace(self.node, self.len, vec![self.held]);
                Some(2)
            }
            _ => None,
        }
    }

    fn roots(&self, keep: &mut dyn FnMut(ValueId)) {
        keep(self.held);
    }
}

/// `[x]`: outputs the length of list x (-1: none).
struct Len;

impl Operation for Len {
    fn name(&self) -> &'static str {
        "len"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        Ok(v.int(v.with_list(cx.args()[0], <[ValueId]>::len).map_or(-1, |n| i64::try_from(n).unwrap())))
    }
}

#[test]
fn what_an_edit_source_holds_survives_a_collection_during_the_evaluation() {
    // The caller's edit source holds a value it interned for an edit it
    // has not applied yet; a collection during the evaluation could not
    // know of it and freed it, and the edit applied an empty slot.
    let e = Engine::new();
    let v = e.values();
    let junk = e.register(Arc::new(Junk));
    let len = e.register(Arc::new(Len));
    e.set_limits(Limits { collect_at_bytes: Some(1), ..Limits::default() });
    let u = e.new_unit();
    let n = e.push(u, len, vec![v.int(0)]);
    let held = made(v, 5, 99);
    let mut src = LaterEdit { held, k: 0, node: n, junk, len };
    let rep = e.evaluate_absorbing(2, &mut src).unwrap();
    assert!(!rep.collections.is_empty(), "collected during the evaluation: {rep:?}");
    assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(5), "{rep:?}");
}
