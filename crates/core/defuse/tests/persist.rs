//! The persisted graph's engine half (docs/spec-persisted-graph.md §2.1,
//! §4): `Engine::export` then `Engine::import` into a fresh engine gives
//! a line that runs nothing with nothing changed, files every record as
//! its commit did, and runs, for any edit, what the exporting engine runs
//! for that edit, with the same products. A refused graph (cut short, a
//! bit flipped, another version, an operation missing) is an error and
//! leaves the engine fresh.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use phitex_defuse::{
    At, Ctx, Effect, Engine, Field, FieldAt, Locality, NodeId, OpId, Operation, Stop, UnitId, ValueId, Values,
};

type Log = Arc<Mutex<Vec<u64>>>;

fn note(log: &Log, cx: &Ctx<'_>) {
    log.lock().unwrap().push(cx.node().raw());
}

/// `[f, v]`: defines f = v.
struct Def(Log);
impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define(f, v);
        Ok(v)
    }
}

/// `[f, v]`: defines f = v in the scope it stands in.
struct LDef(Log);
impl Operation for LDef {
    fn name(&self) -> &'static str {
        "ldef"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define_with(f, v, Locality::Local);
        Ok(v)
    }
}

/// `[f]`: f := f + 1, a relation (a counter's step).
struct Inc(Log);
impl Operation for Inc {
    fn name(&self) -> &'static str {
        "inc"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        cx.define_rel(Field(cx.args()[0]), 1);
        Ok(ValueId::UNIT)
    }
}

/// `[f, x]`: f := f with x appended (`Rel::Append`, a first use's list).
struct App(Log);
impl Operation for App {
    fn name(&self) -> &'static str {
        "app"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        let items = cx.values().list(vec![cx.args()[1]]);
        cx.define_append_with(Field(cx.args()[0]), items, Locality::Global);
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: outputs what reaches it of f.
struct Read(Log);
impl Operation for Read {
    fn name(&self) -> &'static str {
        "read"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

/// `[f]`: outputs the environment's f.
struct Env(Log);
impl Operation for Env {
    fn name(&self) -> &'static str {
        "env"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        Ok(cx.env(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

/// `[f]`: outputs f's last definition on the line (a loop-back read).
struct Final(Log);
impl Operation for Final {
    fn name(&self) -> &'static str {
        "final"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        Ok(match cx.read_at(Field(cx.args()[0]), At::Final)? {
            FieldAt::Def(x, _) => x,
            _ => ValueId::UNIT,
        })
    }
}

/// `[]`: opens a scope after itself.
struct Open(Log);
impl Operation for Open {
    fn name(&self) -> &'static str {
        "open"
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Open
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        Ok(ValueId::UNIT)
    }
}

/// `[]`: closes a scope before itself.
struct Close(Log);
impl Operation for Close {
    fn name(&self) -> &'static str {
        "close"
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Close
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        Ok(ValueId::UNIT)
    }
}

/// `[s, x]`: contributes x to s.
struct Contrib(Log);
impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        true
    }
}

/// `[s]`: the fold of s's contributions before it.
struct Sum(Log);
impl Operation for Sum {
    fn name(&self) -> &'static str {
        "sum"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        cx.fold(Field(cx.args()[0]), None, None)
    }
    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// `[s]`: folds s's contributions in line order with `next_contribution`,
/// parking at a hole (`Ctx::park`) and going on from its state; defines
/// `total`.
struct Loop(Log);
impl Operation for Loop {
    fn name(&self) -> &'static str {
        "loop"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        let v = cx.values();
        let s = Field(cx.args()[0]);
        let state = match (cx.resumed(), cx.woken_by()) {
            (Some(st), Some(phitex_defuse::Cause::Range { by, .. })) => {
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                if c >= 0 && !cx.before(NodeId::from_raw(c.unsigned_abs()), by) {
                    None
                } else {
                    Some(st)
                }
            }
            (st, _) => st,
        };
        let (mut cur, mut sum): (Option<NodeId>, i64) = match state {
            Some(st) => {
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                (
                    (c >= 0).then(|| NodeId::from_raw(c.unsigned_abs())),
                    v.as_int(parts[1]).unwrap_or(0),
                )
            }
            None => (None, 0),
        };
        while let Some((n, x)) = cx.next_contribution(s, cur)? {
            sum += v.as_int(x).unwrap_or(0);
            cur = Some(n);
        }
        let state = v.list(vec![
            v.int(cur.map_or(-1, |n| i64::try_from(n.raw()).unwrap())),
            v.int(sum),
        ]);
        cx.park(state);
        if cx.stopped_at().is_some() {
            return Ok(ValueId::UNIT);
        }
        cx.define(v.field("total"), v.int(sum));
        Ok(v.int(sum))
    }
    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// `[x]`: outputs x; a sibling a claim takes.
struct Text(Log);
impl Operation for Text {
    fn name(&self) -> &'static str {
        "text"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        Ok(cx.args()[0])
    }
}

/// `[n]`: reads its next n siblings and claims them.
struct Claimer(Log);
impl Operation for Claimer {
    fn name(&self) -> &'static str {
        "claimer"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
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

/// `[k, f]`: a picture: places k children, each reading f at it (before
/// it), and outputs their outputs.
struct Pic(Log, std::sync::OnceLock<OpId>);
impl Operation for Pic {
    fn name(&self) -> &'static str {
        "pic"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        let v = cx.values();
        let k = v.as_int(cx.args()[0]).unwrap();
        let f = cx.args()[1];
        let me = v.int(i64::try_from(cx.node().raw()).unwrap());
        let child = *self.1.get().unwrap();
        let mut outs = Vec::new();
        for i in 0..k {
            let j = cx.child(child, vec![f, me, v.int(i)]);
            outs.push(cx.placed_output_or(j, ValueId::UNIT));
        }
        Ok(v.list(outs))
    }
}

/// `[f, at, i]`: a picture's child: reads f before node `at`, its parent.
struct PicChild(Log);
impl Operation for PicChild {
    fn name(&self) -> &'static str {
        "pic_child"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        note(&self.0, cx);
        let v = cx.values();
        let f = Field(cx.args()[0]);
        let at = NodeId::from_raw(v.as_int(cx.args()[1]).unwrap().unsigned_abs());
        let x = match cx.read_at(f, At::Before(at))? {
            FieldAt::Def(x, _) => x,
            _ => ValueId::UNIT,
        };
        Ok(v.list(vec![x, cx.args()[2]]))
    }
}

#[derive(Clone)]
struct Ops {
    log: Log,
    def: OpId,
    ldef: OpId,
    inc: OpId,
    app: OpId,
    read: OpId,
    env: OpId,
    fin: OpId,
    open: OpId,
    close: OpId,
    contrib: OpId,
    sum: OpId,
    lp: OpId,
    text: OpId,
    claimer: OpId,
    pic: OpId,
}

/// A fresh engine with every operation registered, in one order, the
/// append and the environment set: what a client makes before it builds
/// or loads.
fn setup() -> (Engine, Ops) {
    let e = Engine::new();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let l = || log.clone();
    let pic = Arc::new(Pic(l(), std::sync::OnceLock::new()));
    let ops = Ops {
        def: e.register(Arc::new(Def(l()))),
        ldef: e.register(Arc::new(LDef(l()))),
        inc: e.register(Arc::new(Inc(l()))),
        app: e.register(Arc::new(App(l()))),
        read: e.register(Arc::new(Read(l()))),
        env: e.register(Arc::new(Env(l()))),
        fin: e.register(Arc::new(Final(l()))),
        open: e.register(Arc::new(Open(l()))),
        close: e.register(Arc::new(Close(l()))),
        contrib: e.register(Arc::new(Contrib(l()))),
        sum: e.register(Arc::new(Sum(l()))),
        lp: e.register(Arc::new(Loop(l()))),
        text: e.register(Arc::new(Text(l()))),
        claimer: e.register(Arc::new(Claimer(l()))),
        pic: e.register(pic.clone()),
        log,
    };
    let child = e.register(Arc::new(PicChild(ops.log.clone())));
    pic.1.set(child).unwrap();
    e.set_append(Arc::new(|input: Option<ValueId>, items: ValueId, v: &Values| {
        let mut xs = input
            .and_then(|x| v.with_list(x, <[ValueId]>::to_vec))
            .unwrap_or_default();
        xs.extend(v.with_list(items, <[ValueId]>::to_vec).unwrap_or_default());
        v.list(xs)
    }));
    (e, ops)
}

/// The document: a long counter chain with readers, scopes, a picture
/// reading at its parent, claims, contributions folded and walked by a
/// parking loop, a list grown by appends, an environment read and a
/// loop-back read of a label defined at the end; a second unit reading
/// the first's.
fn build(e: &Engine, o: &Ops, chain: usize) -> (UnitId, UnitId) {
    let v = e.values();
    let f = |s: &str| v.field(s).0;
    e.set_environment([(v.field("envf"), v.int(42))]);
    let u = e.new_unit();
    e.push(u, o.fin, vec![f("label")]);
    e.push(u, o.env, vec![f("envf")]);
    e.push(u, o.def, vec![f("x"), v.int(1)]);
    e.push(u, o.def, vec![f("c"), v.int(0)]);
    for i in 0..chain {
        e.push(u, o.inc, vec![f("c")]);
        if i % 25 == 0 {
            e.push(u, o.read, vec![f("c")]);
            e.push(u, o.contrib, vec![f("s"), v.int(i as i64)]);
        }
        if i % 40 == 0 {
            e.push(u, o.app, vec![f("fonts"), v.str(&format!("font{}", i % 3))]);
            e.push(u, o.read, vec![f("fonts")]);
        }
    }
    e.push(u, o.read, vec![f("x")]);
    e.push(u, o.open, vec![]);
    e.push(u, o.ldef, vec![f("z"), v.int(5)]);
    e.push(u, o.read, vec![f("z")]);
    e.push(u, o.pic, vec![v.int(3), f("x")]);
    e.push(u, o.close, vec![]);
    e.push(u, o.read, vec![f("z")]);
    e.push(u, o.claimer, vec![v.int(2)]);
    e.push(u, o.text, vec![v.str("a")]);
    e.push(u, o.text, vec![v.str("b")]);
    e.push(u, o.text, vec![v.str("c")]);
    e.push(u, o.sum, vec![f("s")]);
    e.push(u, o.lp, vec![f("s")]);
    e.push(u, o.read, vec![f("total")]);
    e.push(u, o.def, vec![f("label"), v.str("L1")]);
    let u2 = e.new_unit();
    e.push(u2, o.read, vec![f("x")]);
    e.push(u2, o.def, vec![f("x"), v.int(9)]);
    e.push(u2, o.read, vec![f("x")]);
    (u, u2)
}

/// What the line holds and gives, by content: the segments' filing and
/// each unit's nodes with their outputs.
fn products(e: &Engine, units: &[UnitId]) -> Vec<String> {
    let v = e.values();
    let mut out = e.filing_lines();
    for &u in units {
        for n in e.order(u) {
            out.push(format!(
                "unit {:?} node {} live {} output {:?}",
                u,
                n.raw(),
                e.is_live(n),
                e.output(n).map(|x| v.digest(x))
            ));
        }
    }
    out
}

fn runs(o: &Ops) -> Vec<u64> {
    let mut l = std::mem::take(&mut *o.log.lock().unwrap());
    l.sort_unstable();
    l
}

/// Exports `e` and imports it into a fresh engine made as `setup` makes
/// one.
fn round_trip(e: &Engine) -> (Engine, Ops) {
    let bytes = e.export(&[]);
    let (f, o) = setup();
    let got = f.import(&bytes).expect("the graph imports");
    assert_eq!(
        got.stats.not_holding, 0,
        "every record's reads hold as filed: {:?}",
        got.stats
    );
    (f, o)
}

fn assert_same(a: &[String], b: &[String], what: &str) {
    if a != b {
        let first = a
            .iter()
            .zip(b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!(
            "{what}: the lines differ first at {first} of {} / {}:\n  exported {:?}\n  imported {:?}",
            a.len(),
            b.len(),
            a.get(first),
            b.get(first)
        );
    }
}

/// After an import: the line as filed is the exporting engine's, every
/// read holds, the invariants hold, and an evaluation with nothing changed
/// runs nothing.
#[test]
fn an_import_runs_nothing_and_files_the_line_as_it_stood() {
    for threads in [1, 8] {
        let (a, oa) = setup();
        let (u, u2) = build(&a, &oa, 400);
        a.evaluate(threads).unwrap();
        // (a second evaluation: priors, a settled loop-back)
        a.evaluate(threads).unwrap();
        runs(&oa);
        let (b, ob) = round_trip(&a);
        assert!(b.check_invariants().is_empty(), "{:?}", b.check_invariants());
        let rc = b.verify_reads();
        assert_eq!(rc.failed, 0, "{rc:?}");
        assert!(rc.reads > 50, "{rc:?}");
        // (the graph has what the tests are about: prior records, reads of
        // every kind a run makes here)
        let p = a.persist_probe(None);
        assert!(p.priors > 0 && p.contribs > 0, "{}", p.line());
        for (k, what) in [
            (0, "me"),
            (1, "before"),
            (4, "final"),
            (5, "env"),
            (6, "fold"),
            (7, "next"),
            (8, "placed"),
            (9, "following"),
        ] {
            assert!(p.reads_by_kind[k] > 0, "no read of kind {what}: {}", p.line());
        }
        assert_eq!(rc, a.verify_reads());
        assert_same(&products(&a, &[u, u2]), &products(&b, &[u, u2]), "the import");
        let r = b.evaluate(threads).unwrap();
        assert_eq!(r.runs, 0, "nothing changed, nothing runs (-j{threads}): {r:?}");
        assert!(runs(&ob).is_empty());
        assert_same(&products(&a, &[u, u2]), &products(&b, &[u, u2]), "after an evaluation");
    }
}

/// An edit after an import runs the nodes the same edit runs in the
/// exporting engine, and gives the same products: a definition changed
/// and changed back (a prior record standing again), a step of the chain,
/// a contribution, a claimed sibling removed, a node inserted, the label
/// read back, the environment, a picture's input; and an engine imported
/// from an imported one after edits.
#[test]
fn an_edit_after_an_import_runs_what_it_runs_before() {
    let (a, oa) = setup();
    let (u, u2) = build(&a, &oa, 300);
    a.evaluate(1).unwrap();
    runs(&oa);
    let (b, ob) = round_trip(&a);
    let mut engines = vec![(a, oa), (b, ob)];
    type Edit = Box<dyn Fn(&Engine, &Ops, &[NodeId])>;
    let edits: Vec<(&str, Edit)> = vec![
        (
            "x := 7",
            Box::new(|e, o, line| {
                let v = e.values();
                let n = line.iter().copied().find(|&n| is(e, n, "def", "x")).unwrap();
                e.replace(n, o.def, vec![v.field("x").0, v.int(7)]);
            }),
        ),
        (
            "x := 1 again",
            Box::new(|e, o, line| {
                let v = e.values();
                let n = line.iter().copied().find(|&n| is(e, n, "def", "x")).unwrap();
                e.replace(n, o.def, vec![v.field("x").0, v.int(1)]);
            }),
        ),
        (
            "a step of the chain set",
            Box::new(|e, o, line| {
                let v = e.values();
                let n = line.iter().copied().filter(|&n| op_of(e, n) == "inc").nth(120).unwrap();
                e.replace(n, o.def, vec![v.field("c").0, v.int(1000)]);
            }),
        ),
        (
            "a contribution",
            Box::new(|e, o, line| {
                let v = e.values();
                let n = line
                    .iter()
                    .copied()
                    .filter(|&n| op_of(e, n) == "contrib")
                    .nth(3)
                    .unwrap();
                e.replace(n, o.contrib, vec![v.field("s").0, v.int(-500)]);
            }),
        ),
        (
            "a claimed sibling removed",
            Box::new(|e, _, line| {
                let n = line.iter().copied().filter(|&n| op_of(e, n) == "text").nth(1).unwrap();
                e.remove(n);
            }),
        ),
        (
            "a read inserted before the picture",
            Box::new(|e, o, line| {
                let v = e.values();
                let n = line.iter().copied().find(|&n| op_of(e, n) == "pic").unwrap();
                e.insert_before(n, o.ldef, vec![v.field("x").0, v.int(77)]);
            }),
        ),
        (
            "the label read back",
            Box::new(|e, o, line| {
                let v = e.values();
                let n = line.iter().copied().find(|&n| is(e, n, "def", "label")).unwrap();
                e.replace(n, o.def, vec![v.field("label").0, v.str("L2")]);
            }),
        ),
        (
            "the environment",
            Box::new(|e, _, _| {
                let v = e.values();
                e.update_environment(&[(v.field("envf"), Some(v.int(43)))]);
            }),
        ),
        (
            "a font used first elsewhere",
            Box::new(|e, o, line| {
                let v = e.values();
                let n = line.iter().copied().filter(|&n| op_of(e, n) == "app").nth(2).unwrap();
                e.replace(n, o.app, vec![v.field("fonts").0, v.str("font9")]);
            }),
        ),
    ];
    for (k, (what, edit)) in edits.iter().enumerate() {
        let mut got: Vec<(Vec<u64>, Vec<String>)> = Vec::new();
        for (e, o) in &engines {
            let line = e.order(u);
            edit(e, o, &line);
            let r = e.evaluate(1).unwrap();
            assert!(e.check_invariants().is_empty(), "{what}: {:?}", e.check_invariants());
            let _ = r;
            got.push((runs(o), products(e, &[u, u2])));
        }
        assert!(!got[0].0.is_empty(), "{what}: the edit ran nothing");
        for (j, g) in got.iter().enumerate().skip(1) {
            assert_eq!(
                got[0].0, g.0,
                "{what}: engine {j} ran other nodes than the exporting one"
            );
            assert_same(&got[0].1, &g.1, what);
        }
        // (half way: the imported engine exported and imported again)
        if k == 3 {
            let (c, oc) = round_trip(&engines[1].0);
            let r = c.evaluate(1).unwrap();
            assert_eq!(r.runs, 0, "{r:?}");
            runs(&oc);
            engines.push((c, oc));
        }
    }
}

/// The same at 8 threads: the products after each edit are the
/// exporting engine's (which runs a schedule picks may differ).
#[test]
fn an_edit_after_an_import_gives_the_same_products_at_8_threads() {
    let (a, oa) = setup();
    let (u, u2) = build(&a, &oa, 600);
    a.evaluate(8).unwrap();
    let (b, ob) = round_trip(&a);
    let engines = [(a, oa), (b, ob)];
    for step in 0..4 {
        let mut got = Vec::new();
        for (e, o) in &engines {
            let v = e.values();
            let line = e.order(u);
            match step {
                0 => {
                    let n = line.iter().copied().find(|&n| is(e, n, "def", "x")).unwrap();
                    e.replace(n, o.def, vec![v.field("x").0, v.int(3)]);
                }
                1 => {
                    let n = line.iter().copied().filter(|&n| op_of(e, n) == "inc").nth(10).unwrap();
                    e.remove(n);
                }
                2 => {
                    let n = line
                        .iter()
                        .copied()
                        .filter(|&n| op_of(e, n) == "contrib")
                        .nth(1)
                        .unwrap();
                    e.insert_after(n, o.contrib, vec![v.field("s").0, v.int(5)]);
                }
                _ => {
                    let n = line.iter().copied().find(|&n| is(e, n, "def", "label")).unwrap();
                    e.replace(n, o.def, vec![v.field("label").0, v.str("L3")]);
                }
            }
            e.evaluate(8).unwrap();
            assert!(e.check_invariants().is_empty());
            got.push(products(e, &[u, u2]));
            runs(o);
        }
        assert_same(&got[0], &got[1], &format!("step {step} at -j8"));
    }
}

fn op_of(e: &Engine, n: NodeId) -> &'static str {
    e.node_info(n).map_or("", |i| i.op)
}

fn is(e: &Engine, n: NodeId, op: &str, field: &str) -> bool {
    e.node_info(n)
        .is_some_and(|i| i.op == op && i.args.first().copied() == Some(e.values().field(field).0))
}

/// A graph cut short anywhere, with a bit flipped anywhere, of another
/// version or naming an operation the engine lacks is refused, and the
/// engine is left fresh: the whole graph imports into it after.
#[test]
fn a_corrupt_or_cut_graph_is_refused_and_leaves_the_engine_fresh() {
    let (a, oa) = setup();
    build(&a, &oa, 100);
    a.evaluate(2).unwrap();
    let bytes = a.export(&[]);
    let (b, _ob) = setup();
    let n = bytes.len();
    let mut cuts: Vec<usize> = (0..n).step_by(n / 97 + 1).collect();
    cuts.extend([0, 1, 7, 8, 12, 16, 24, 40, n - 1]);
    for cut in cuts {
        assert!(b.import(&bytes[..cut]).is_err(), "cut at {cut} of {n}");
    }
    let flips = AtomicU64::new(0);
    for at in (0..n).step_by(n / 131 + 1).chain([9, 20, n - 1]) {
        for bit in [0, 3, 7] {
            let mut x = bytes.clone();
            x[at] ^= 1 << bit;
            assert!(b.import(&x).is_err(), "a bit flipped at {at}");
            flips.fetch_add(1, Ordering::Relaxed);
        }
    }
    let mut x = bytes.clone();
    x[8] = 99;
    assert!(b.import(&x).unwrap_err().to_string().contains("version"));
    assert_eq!(b.live_nodes(), 0, "a refused graph leaves no node");
    // (an engine without one of the operations)
    let c = Engine::new();
    let l: Log = Arc::new(Mutex::new(Vec::new()));
    c.register(Arc::new(Def(l.clone())));
    assert!(c.import(&bytes).unwrap_err().to_string().contains("not registered"));
    // (into the engine that refused them all, the whole graph)
    let got = b.import(&bytes).unwrap();
    assert_eq!(got.stats.not_holding, 0);
    let r = b.evaluate(2).unwrap();
    assert_eq!(r.runs, 0, "{r:?}");
    // (and not twice: an engine with a line is not fresh)
    assert!(b.import(&bytes).is_err());
}

/// The roots a client passes come back as the importing store's values,
/// by content.
#[test]
fn roots_come_back_by_content() {
    let (a, oa) = setup();
    build(&a, &oa, 10);
    a.evaluate(1).unwrap();
    let v = a.values();
    let roots = vec![
        v.str("page"),
        v.list(vec![v.int(1), v.str("x")]),
        v.seq_concat(v.seq_unit(v.int(5)), v.seq_unit(v.str("y"))),
    ];
    let bytes = a.export(&roots);
    let (b, _) = setup();
    let got = b.import(&bytes).unwrap();
    assert_eq!(got.roots.len(), 3);
    for (x, y) in roots.iter().zip(&got.roots) {
        assert_eq!(v.digest(*x), b.values().digest(*y));
    }
}
