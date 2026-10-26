//! Scopes: brackets on the line decide which definitions reach where.
//! Every test here passes before any client code is touched.

use std::sync::Arc;

use phitex_defuse::{Brackets, Ctx, Effect, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

/// `[f, v]`: defines f = v locally; outputs v.
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

/// `[f, v]`: defines f = v globally.
struct DefGlobal;

impl Operation for DefGlobal {
    fn name(&self) -> &'static str {
        "def_global"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define_with(f, v, Locality::Global);
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

/// `[]`: opens one scope after itself (its effect is static).
struct Open;

impl Operation for Open {
    fn name(&self) -> &'static str {
        "open"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Open
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// `[]`: closes one scope before itself.
struct Close;

impl Operation for Close {
    fn name(&self) -> &'static str {
        "close"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Close
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// `[]`: nothing (a replacement for a bracket node in an edit).
struct Noop;

impl Operation for Noop {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// `[f, v]`: opens one scope and defines f = v inside it (`Locality::Scope`).
struct DefInside;

impl Operation for DefInside {
    fn name(&self) -> &'static str {
        "def_inside"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Open
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define_with(f, v, Locality::Scope);
        Ok(v)
    }
}

/// `[f]`: closes one scope before itself, then reads f: what a node whose
/// text begins with `}` sees.
struct CloseThenGet;

impl Operation for CloseThenGet {
    fn name(&self) -> &'static str {
        "close_then_get"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Close
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        Ok(cx.read(f)?.unwrap_or(ValueId::UNIT))
    }
}

/// `[f, v, closes, opens]`: its brackets are known only by running:
/// closes `closes` before itself, defines f = v locally, opens `opens`.
struct Pending;

impl Operation for Pending {
    fn name(&self) -> &'static str {
        "pending"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let (f, x) = (Field(cx.args()[0]), cx.args()[1]);
        let closes = v.as_int(cx.args()[2]).unwrap() as u8;
        let opens = v.as_int(cx.args()[3]).unwrap() as u8;
        cx.set_effect(Effect::Scope(Brackets {
            closes,
            opens,
            before: true,
        }));
        cx.define(f, x);
        Ok(x)
    }
}

/// `[f, a, b, c]`: opens two scopes after itself and defines f = a
/// outside them, b in the first, c in the second: a run that leaves two
/// groups open with a value in each.
struct DefFrames;

impl Operation for DefFrames {
    fn name(&self) -> &'static str {
        "def_frames"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Scope(Brackets {
            opens: 2,
            ..Brackets::default()
        })
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        let (a, b, c) = (cx.args()[1], cx.args()[2], cx.args()[3]);
        cx.define_with(f, a, Locality::Local);
        cx.define_with(f, b, Locality::ScopeAt(0));
        cx.define_with(f, c, Locality::ScopeAt(1));
        Ok(ValueId::UNIT)
    }
}

/// `[]`: outputs the depth it starts at.
struct Depth;

impl Operation for Depth {
    fn name(&self) -> &'static str {
        "depth"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let d = cx.depth()?;
        Ok(cx.values().int(i64::from(d)))
    }
}

struct Ops {
    def: OpId,
    def_global: OpId,
    get: OpId,
    open: OpId,
    close: OpId,
    noop: OpId,
    def_inside: OpId,
    close_then_get: OpId,
    pending: OpId,
    depth: OpId,
    def_frames: OpId,
}

fn engine() -> (Engine, Ops) {
    let e = Engine::new();
    let ops = Ops {
        def: e.register(Arc::new(Def)),
        def_global: e.register(Arc::new(DefGlobal)),
        get: e.register(Arc::new(Get)),
        open: e.register(Arc::new(Open)),
        close: e.register(Arc::new(Close)),
        noop: e.register(Arc::new(Noop)),
        def_inside: e.register(Arc::new(DefInside)),
        close_then_get: e.register(Arc::new(CloseThenGet)),
        pending: e.register(Arc::new(Pending)),
        depth: e.register(Arc::new(Depth)),
        def_frames: e.register(Arc::new(DefFrames)),
    };
    (e, ops)
}

fn int(e: &Engine, n: NodeId) -> i64 {
    e.values().as_int(e.output(n).expect("ran")).expect("int")
}

#[test]
fn a_local_definition_dies_at_its_close() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    let inside = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, inside), 2);
    assert_eq!(int(&e, after), 1, "the definition inside the scope does not reach past its close");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_global_definition_survives_its_close() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def_global, vec![x, v.int(2)]);
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, after), 2);
}

#[test]
fn a_global_under_a_local_returns_when_the_local_closes() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def_global, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    let inside = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(2).unwrap();
    assert_eq!((int(&e, inside), int(&e, after)), (2, 1));
}

#[test]
fn scopes_nest() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(3)]);
    let d2 = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    let d1 = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    let d0 = e.push(u, o.get, vec![x]);
    let dep = e.push(u, o.depth, vec![]);
    e.evaluate(2).unwrap();
    assert_eq!((int(&e, d2), int(&e, d1), int(&e, d0)), (3, 2, 1));
    assert_eq!(int(&e, dep), 0);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_definition_inside_the_scope_a_node_opens() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.def_inside, vec![x, v.int(5)]);
    let inside = e.push(u, o.get, vec![x]);
    let dep = e.push(u, o.depth, vec![]);
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(2).unwrap();
    assert_eq!((int(&e, inside), int(&e, dep), int(&e, after)), (5, 1, 1));
}

#[test]
fn a_node_defining_a_field_in_each_scope_it_opens() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    let fr = e.push(u, o.def_frames, vec![x, v.int(2), v.int(3), v.int(4)]);
    let d2 = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    let d1 = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    let d0 = e.push(u, o.get, vec![x]);
    e.evaluate(2).unwrap();
    assert_eq!((int(&e, d2), int(&e, d1), int(&e, d0)), (4, 3, 2));
    e.replace(fr, o.def_frames, vec![x, v.int(2), v.int(6), v.int(4)]);
    e.evaluate(2).unwrap();
    assert_eq!((int(&e, d2), int(&e, d1), int(&e, d0)), (4, 6, 2), "a change of one part re-asks the readers that see it");
    let bad = e.check_invariants();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn closes_before_a_node_apply_to_its_reads() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    let g = e.push(u, o.close_then_get, vec![x]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, g), 1, "a node that begins by closing the scope reads the outer value");
}

#[test]
fn an_edit_of_a_bracket_reasks_the_readers_after_it() {
    let (e, o) = engine();
    let v = e.values();
    let (x, y) = (v.field("x").0, v.field("y").0);
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.def, vec![y, v.int(7)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    let close = e.push(u, o.close, vec![]);
    let gx = e.push(u, o.get, vec![x]);
    let gy = e.push(u, o.get, vec![y]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, gx), 1);

    e.replace(close, o.noop, vec![]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(int(&e, gx), 2, "with the close gone the inner definition reaches on");
    assert_eq!(int(&e, gy), 7);
    assert_eq!(r.nodes_run, 2, "the edited node and the one reader whose answer moved: {r:?}");

    e.replace(close, o.close, vec![]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, gx), 1);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_pending_effect_is_filed_by_the_run() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    // closes 1 before itself, defines x = 3 at the outer depth, opens 1
    e.push(u, o.pending, vec![x, v.int(3), v.int(1), v.int(1)]);
    let inside = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(4).unwrap();
    assert_eq!(int(&e, inside), 3);
    assert_eq!(int(&e, after), 3, "x = 3 was made outside the scope the node opened");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn scopes_reach_across_segments() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    for _ in 0..500 {
        e.push(u, o.noop, vec![]);
    }
    e.push(u, o.def, vec![x, v.int(2)]);
    for _ in 0..500 {
        e.push(u, o.noop, vec![]);
    }
    let inside = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    for _ in 0..500 {
        e.push(u, o.noop, vec![]);
    }
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(4).unwrap();
    assert_eq!((int(&e, inside), int(&e, after)), (2, 1));
    assert!(e.check_invariants().is_empty());
}

#[test]
fn an_unmatched_close_at_depth_zero_is_harmless() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.close, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    let g = e.push(u, o.get, vec![x]);
    let dep = e.push(u, o.depth, vec![]);
    e.evaluate(2).unwrap();
    assert_eq!((int(&e, g), int(&e, dep)), (2, 0));
}

#[test]
fn many_scopes_side_by_side() {
    // 2000 scopes each with its own local, read inside and after: the
    // visibility walk is local, never over the whole line.
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(0)]);
    let mut pairs = Vec::new();
    for i in 1..=2000 {
        e.push(u, o.open, vec![]);
        e.push(u, o.def, vec![x, v.int(i)]);
        let inside = e.push(u, o.get, vec![x]);
        e.push(u, o.close, vec![]);
        let after = e.push(u, o.get, vec![x]);
        pairs.push((i, inside, after));
    }
    let t = std::time::Instant::now();
    e.evaluate(4).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
    for (i, inside, after) in pairs {
        assert_eq!(int(&e, inside), i);
        assert_eq!(int(&e, after), 0);
    }
    assert!(e.check_invariants().is_empty());
}
