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

/// `[f, c]`: f := the value reaching it + c, globally (a relation).
struct RelGlobal;

impl Operation for RelGlobal {
    fn name(&self) -> &'static str {
        "rel_global"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let c = cx.values().as_int(cx.args()[1]).unwrap_or(0);
        cx.define_rel_with(Field(cx.args()[0]), c, Locality::Global);
        Ok(ValueId::UNIT)
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

/// `[f, a, g, b]`: defines f = a locally and g = b globally, then
/// closes the scope it runs in (its closes after its definitions): a run
/// that ends at the `\\endgroup` it closes its group with.
struct DefThenClose;

impl Operation for DefThenClose {
    fn name(&self) -> &'static str {
        "def_then_close"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Scope(Brackets {
            closes: 1,
            opens: 0,
            before: false,
        })
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let a = cx.args().to_vec();
        cx.define(Field(a[0]), a[1]);
        cx.define_with(Field(a[2]), a[3], Locality::Global);
        Ok(ValueId::UNIT)
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
    rel_global: OpId,
    def_then_close: OpId,
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
        rel_global: e.register(Arc::new(RelGlobal)),
        def_then_close: e.register(Arc::new(DefThenClose)),
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
    assert_eq!(
        int(&e, after),
        1,
        "the definition inside the scope does not reach past its close"
    );
    assert!(e.check_invariants().is_empty());
}

/// A node whose closes come after its definitions (`before: false`): its
/// local definition is the scope's and dies with it, its global one
/// stands; a reader inside the scope before it is not touched.
#[test]
fn a_closing_nodes_local_definition_dies_with_the_scope_it_closes() {
    let (e, o) = engine();
    let v = e.values();
    let (x, y) = (v.field("x").0, v.field("y").0);
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.def, vec![y, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    let inside = e.push(u, o.get, vec![x]);
    e.push(u, o.def_then_close, vec![x, v.int(3), y, v.int(4)]);
    let after_x = e.push(u, o.get, vec![x]);
    let after_y = e.push(u, o.get, vec![y]);
    let dep = e.push(u, o.depth, vec![]);
    for threads in [1, 4] {
        e.evaluate(threads).unwrap();
        assert_eq!(int(&e, inside), 2);
        assert_eq!(
            int(&e, after_x),
            1,
            "the closing node's local definition does not reach past its close"
        );
        assert_eq!(int(&e, after_y), 4, "its global definition stands");
        assert_eq!(int(&e, dep), 0);
    }
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
    assert_eq!(
        (int(&e, d2), int(&e, d1), int(&e, d0)),
        (4, 6, 2),
        "a change of one part re-asks the readers that see it"
    );
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
    assert_eq!(
        int(&e, g),
        1,
        "a node that begins by closing the scope reads the outer value"
    );
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
    assert_eq!(
        r.nodes_run, 2,
        "the edited node and the one reader whose answer moved: {r:?}"
    );

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

/// A global relation inside a scope adds to the value reaching it, the
/// innermost binding there, and stands past the close: the second step
/// counts the first though both are in scopes closed before it reads.
#[test]
fn a_global_relation_inside_a_scope_survives_its_close() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(10)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.rel_global, vec![x, v.int(1)]);
    let inside = e.push(u, o.get, vec![x]);
    e.push(u, o.close, vec![]);
    e.push(u, o.open, vec![]);
    e.push(u, o.rel_global, vec![x, v.int(1)]);
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(4).unwrap();
    assert_eq!(int(&e, inside), 11);
    assert_eq!(int(&e, after), 12, "both steps stand past their scopes");
    assert!(e.check_invariants().is_empty());
}

/// A global relation adds to the innermost binding it sees (a global
/// `\advance` of a register an open group bound locally, §279): 5 + 1
/// stands after the close, not 10 + 1.
#[test]
fn a_global_relation_adds_to_the_local_binding_it_sees() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(10)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(5)]);
    e.push(u, o.rel_global, vec![x, v.int(1)]);
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![x]);
    e.evaluate(4).unwrap();
    assert_eq!(int(&e, after), 6);
    assert!(e.check_invariants().is_empty());
}

/// `[k, shown]`: predicted to open a scope, its text shows one, and its
/// run opens none (`\\begin{x}` in a short verbatim). The same shape
/// (`shown`) through an edit: the same effect.
struct Shows;

impl Operation for Shows {
    fn name(&self) -> &'static str {
        "shows"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Scope(Brackets {
            closes: 0,
            opens: 1,
            before: false,
        })
    }

    fn same_effect(&self, old: &[ValueId], new: &[ValueId], _: &Values) -> bool {
        old[1] == new[1]
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.set_effect(Effect::None);
        Ok(cx.args()[0])
    }
}

/// An edit that leaves what a node's text shows files no brackets its last
/// run refuted, so no walk of the readers after it, at the edit or at the
/// run's commit; one that changes it files the prediction, as ever, and
/// the run takes it back.
#[test]
fn an_edit_keeps_the_brackets_its_last_run_found_where_the_text_shows_the_same() {
    let eng = Engine::new();
    let shows = eng.register(Arc::new(Shows));
    let v = eng.values();
    let (one, two, three) = (v.int(1), v.int(2), v.int(3));
    let (a, b) = (v.int(10), v.int(11));
    let u = eng.new_unit();
    let n = eng.push(u, shows, vec![one, a]);
    eng.evaluate(1).unwrap();
    let c0 = eng.scope_changes();
    eng.replace(n, shows, vec![two, a]);
    assert_eq!(eng.scope_changes(), c0, "the edit files nothing");
    eng.evaluate(1).unwrap();
    assert_eq!(eng.scope_changes(), c0, "the run takes nothing back");
    eng.replace(n, shows, vec![three, b]);
    assert_eq!(eng.scope_changes(), c0 + 1, "another shape: the prediction filed");
    eng.evaluate(1).unwrap();
    assert_eq!(eng.scope_changes(), c0 + 2, "and taken back by the run");
}

/// `[f]`: outputs the definition of f reaching the end of its unit.
struct EndGet;

impl Operation for EndGet {
    fn name(&self) -> &'static str {
        "end_get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        Ok(match cx.read_at(f, phitex_defuse::At::End(cx.unit()))? {
            phitex_defuse::FieldAt::Def(x, _) => x,
            _ => ValueId::UNIT,
        })
    }
}

/// A close that goes asks again the readers a walk meets that are not
/// filed at positions: the one at the unit's end and the relation after
/// it (`x := x + 10`, its input read through the brackets); filed at
/// positions, only the plain reader was, and the end reader kept the
/// inner definition hidden.
#[test]
fn a_close_that_goes_asks_the_end_reader_and_the_relation_after_it() {
    let (e, o) = engine();
    let end_get = e.register(Arc::new(EndGet));
    let v = e.values();
    let (x, c, z) = (v.field("x").0, v.field("c").0, v.field("z").0);
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.def, vec![c, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    e.push(u, o.def, vec![c, v.int(2)]);
    let closing = |k: i64| vec![z, v.int(9), v.int(k), v.int(0)];
    let p = e.push(u, o.pending, closing(1));
    let gx = e.push(u, o.get, vec![x]);
    e.push(u, o.rel_global, vec![c, v.int(10)]);
    let gc = e.push(u, o.get, vec![c]);
    let ex = e.push(u, end_get, vec![x]);
    let check = |e: &Engine, closes: bool| {
        let (xv, cv) = if closes { (1, 11) } else { (2, 12) };
        assert_eq!(int(e, gx), xv, "x after it (closes {closes})");
        assert_eq!(int(e, ex), xv, "x at the unit's end (closes {closes})");
        assert_eq!(int(e, gc), cv, "the relation's value after it (closes {closes})");
        assert!(e.check_invariants().is_empty());
    };
    e.evaluate(1).unwrap();
    check(&e, true);
    for threads in [1, 4] {
        e.replace(p, o.pending, closing(0));
        e.evaluate(threads).unwrap();
        check(&e, false);
        e.replace(p, o.pending, closing(1));
        e.evaluate(threads).unwrap();
        check(&e, true);
    }
}

/// `[k]`: closes `k` scopes before itself (known only by running) and
/// places after itself an opener whose text names `k`, made anew at each
/// change of it: one commit files its own brackets and then its new
/// child's, so its walk finds the scopes moved since its own were filed
/// (a race, though at one thread) and asks every reader after it.
struct CloseAndOpen(OpId);

impl Operation for CloseAndOpen {
    fn name(&self) -> &'static str {
        "close_and_open"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let k = cx.values().as_int(cx.args()[0]).unwrap();
        cx.set_effect(Effect::Scope(Brackets {
            closes: k as u8,
            opens: 0,
            before: true,
        }));
        let arg = cx.args()[0];
        cx.follow(self.0, vec![arg]);
        Ok(ValueId::UNIT)
    }
}

/// `[k]`: opens one scope after itself; `k` only names it.
struct OpenNamed;

impl Operation for OpenNamed {
    fn name(&self) -> &'static str {
        "open_named"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Open
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// The walk after a race (the scopes moved since the node's brackets were
/// filed: here by its own new child's) asks every reader after the change
/// by its answer, and with them the end reader, which is not filed at a
/// position: it kept the inner definition hidden.
#[test]
fn a_walk_after_a_race_asks_the_end_reader_too() {
    let (e, o) = engine();
    let end_get = e.register(Arc::new(EndGet));
    let open_named = e.register(Arc::new(OpenNamed));
    let cao = e.register(Arc::new(CloseAndOpen(open_named)));
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![x, v.int(2)]);
    let p = e.push(u, cao, vec![v.int(0)]);
    e.push(u, o.close, vec![]);
    let gx = e.push(u, o.get, vec![x]);
    let ex = e.push(u, end_get, vec![x]);
    // (closes 0: the inner scope stays open, the child's opener closed by
    // the close; closes 1: the inner scope closed, so is the child's)
    let check = |e: &Engine, closes: bool| {
        let xv = if closes { 1 } else { 2 };
        assert_eq!(int(e, gx), xv, "x after both (closes {closes})");
        assert_eq!(int(e, ex), xv, "x at the unit's end (closes {closes})");
        assert!(e.check_invariants().is_empty());
    };
    e.evaluate(1).unwrap();
    check(&e, false);
    for threads in [1, 4] {
        e.replace(p, cao, vec![v.int(1)]);
        e.evaluate(threads).unwrap();
        check(&e, true);
        e.replace(p, cao, vec![v.int(0)]);
        e.evaluate(threads).unwrap();
        check(&e, false);
    }
}

/// `[cut, predict]`: a piece predicted to close its scope (its effect is
/// static). With `cut` 0 its run closes; with 1 it is cut before its
/// close: it closes nothing itself and places its rest, which closes;
/// with `predict` 1 the rest's brackets are filed with the piece's commit
/// (`Ctx::predict_placed_effect`), not left to the rest's run.
struct CutPiece(OpId);

impl Operation for CutPiece {
    fn name(&self) -> &'static str {
        "cut_piece"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Close
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let (cut, predict) = (v.as_int(cx.args()[0]) == Some(1), v.as_int(cx.args()[1]) == Some(1));
        if !cut {
            cx.set_effect(Effect::Close);
            return Ok(ValueId::UNIT);
        }
        cx.set_effect(Effect::None);
        let i = cx.follow(self.0, vec![]);
        if predict {
            cx.predict_placed_effect(i, Effect::Close);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[]`: a cut piece's rest: its brackets are its run's (it closes).
struct PieceRest;

impl Operation for PieceRest {
    fn name(&self) -> &'static str {
        "piece_rest"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.set_effect(Effect::Close);
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: outputs the definition of f reaching it, counting its runs.
struct CountGet(Arc<std::sync::atomic::AtomicUsize>);

impl Operation for CountGet {
    fn name(&self) -> &'static str {
        "count_get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

/// A piece whose run is now cut before its close places its rest in its
/// commit: filed with the close the piece's text predicted, the depth
/// after the rest is as it was, and the reader after it, which ran on
/// that depth, is not run on a scope left open until the rest's run (the
/// beamer user guide at -j4: each such cut filed the piece's close away
/// and the rest's run filed it back, the readers after woken twice). Left
/// to the rest's run, the reader is woken: the test sees the difference.
#[test]
fn a_cut_pieces_rest_is_filed_with_the_close_its_piece_predicted() {
    for predict in [1, 0] {
        let (e, o) = engine();
        let rest = e.register(Arc::new(PieceRest));
        let cut = e.register(Arc::new(CutPiece(rest)));
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let get = e.register(Arc::new(CountGet(n.clone())));
        let v = e.values();
        let x = v.field("x").0;
        let u = e.new_unit();
        e.push(u, o.def, vec![x, v.int(1)]);
        e.push(u, o.open, vec![]);
        e.push(u, o.def, vec![x, v.int(2)]);
        let c = e.push(u, cut, vec![v.int(0), v.int(predict)]);
        let r = e.push(u, get, vec![x]);
        e.evaluate(1).unwrap();
        assert_eq!(int(&e, r), 1, "the piece closed the scope");
        let n0 = n.load(std::sync::atomic::Ordering::Relaxed);
        // (its run now cut before its close: a rest placed)
        e.replace(c, cut, vec![v.int(1), v.int(predict)]);
        let rep = e.evaluate(1).unwrap();
        // (woken: run, or found standing by its reads when it came up)
        let woken = rep.rerun_causes.iter().any(|(m, _)| *m == r)
            || rep.runs_by_op.iter().any(|(name, k)| *name == "count_get" && *k > 0);
        assert_eq!(e.order(u).len(), 6, "the rest placed");
        assert_eq!(int(&e, r), 1, "the rest closed the scope");
        assert!(e.check_invariants().is_empty());
        let again = n.load(std::sync::atomic::Ordering::Relaxed) - n0;
        if predict == 1 {
            assert_eq!(again, 0, "the reader after the new rest stands");
            assert!(!woken, "the reader after the new rest is not woken: {rep:?}");
        } else {
            assert!(
                woken,
                "left to the rest's run, the reader was woken on the open scope: {rep:?}"
            );
        }
    }
}

/// `[open]`: with `open` 1, opens one scope (its brackets known only by
/// running) and defines `g` = 1 inside it, as TeX's piece leaving a
/// group open defines its scope kind there; with 0, neither.
struct SwitchOpen(Field);

impl Operation for SwitchOpen {
    fn name(&self) -> &'static str {
        "switch_open"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if cx.values().as_int(cx.args()[0]) == Some(1) {
            cx.set_effect(Effect::Open);
            let one = cx.values().int(1);
            cx.define_with(self.0, one, Locality::Scope);
        } else {
            cx.set_effect(Effect::None);
        }
        Ok(ValueId::UNIT)
    }
}

/// A node that comes to open a scope its close after it already closes,
/// defining inside it a field it did not define before: that definition
/// dies at the close, so the readers of the field after the close are
/// not asked again (an opener's scope kind: every reader of it to the
/// end of the line was asked at each opener's first commit, 1 040 s of
/// biblatex's -j4 build, mc 80580); a reader inside the scope sees the
/// definition, those after it none, as before.
#[test]
fn a_definition_inside_a_scope_a_node_comes_to_open_asks_no_reader_after_its_close() {
    let (e, o) = engine();
    let v = e.values();
    let g = v.field("g");
    let open = e.register(Arc::new(SwitchOpen(g)));
    let u = e.new_unit();
    let n = e.push(u, open, vec![v.int(0)]);
    let inside = e.push(u, o.get, vec![g.0]);
    e.push(u, o.close, vec![]);
    // (another opener's definition in its own scope: the walk of `n`'s
    // definition itself stops there, as a later opener's scope kind
    // stops it)
    e.push(u, open, vec![v.int(1)]);
    e.push(u, o.close, vec![]);
    let after: Vec<NodeId> = (0..500).map(|_| e.push(u, o.get, vec![g.0])).collect();
    e.evaluate(1).unwrap();
    assert_eq!(e.output(inside), Some(ValueId::UNIT));
    e.replace(n, open, vec![v.int(1)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(e.output(inside), Some(v.int(1)), "the reader inside the scope sees it");
    assert!(
        after.iter().all(|&r| e.output(r) == Some(ValueId::UNIT)),
        "the readers after the close see none"
    );
    assert!(
        rep.walked < 100,
        "{} readers walked for a definition no reader after the close sees",
        rep.walked
    );
    assert!(e.check_invariants().is_empty());
}

/// A node that comes to open a scope no close after it closes (its close
/// not run yet: text run ahead on a guess of the nesting) changes the
/// scopes from it to the line's end. Asked again are the readers of a
/// field whose definition there, or before it in a scope open there, the
/// change can hide or show, not every reader on the way: the readers of a
/// field defined only globally see what they saw (biblatex at -j4: 3 to
/// 4 M readers asked at each such change, 2 to 5 % of them of such a
/// field, ~15 s each, accl 25435). The readers that do see another
/// answer get it: one of a local definition inside the outer scope,
/// which the close after the node now leaves open, and one inside the
/// node's new scope.
#[test]
fn a_change_of_brackets_asks_only_the_readers_whose_scopes_can_move() {
    let (e, o) = engine();
    let v = e.values();
    let (g, gx, l) = (v.field("g"), v.field("gx"), v.field("l"));
    let open = e.register(Arc::new(SwitchOpen(g)));
    let u = e.new_unit();
    e.push(u, o.def_global, vec![gx.0, v.int(1)]);
    e.push(u, o.open, vec![]);
    e.push(u, o.def, vec![l.0, v.int(5)]);
    let n = e.push(u, open, vec![v.int(0)]);
    let inside = e.push(u, o.get, vec![g.0]);
    let globals: Vec<NodeId> = (0..500).map(|_| e.push(u, o.get, vec![gx.0])).collect();
    e.push(u, o.close, vec![]);
    let after = e.push(u, o.get, vec![l.0]);
    let more: Vec<NodeId> = (0..500).map(|_| e.push(u, o.get, vec![gx.0])).collect();
    e.evaluate(1).unwrap();
    assert_eq!(e.output(after), Some(ValueId::UNIT), "the outer scope closed");
    e.replace(n, open, vec![v.int(1)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(e.output(inside), Some(v.int(1)), "the reader inside the new scope sees it");
    assert_eq!(int(&e, after), 5, "the close ends the new scope, the outer one stays open");
    assert!(globals.iter().chain(&more).all(|&r| int(&e, r) == 1));
    assert!(
        rep.scope_asked < 100,
        "{} readers asked again for a change only a few readers' scopes see",
        rep.scope_asked
    );
    assert!(e.check_invariants().is_empty());
}
