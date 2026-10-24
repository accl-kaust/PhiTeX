//! The clean core's `DEFUSE_RECORD` recording: a small program with
//! loop-back fields, an accumulator, the environment (supplied and from a
//! provider), edits between evaluations and edits absorbed during one.
//! Then, as a client that builds its format first does, more engines in
//! the same process: runs that stop, park and resume more than once, read
//! a range that stops, place a continuation through `follow_after`; a
//! provider and a hook whose answers change between evaluations, with
//! `refresh_where`; the calls that tune an engine (`set_stable`,
//! `set_heights`, `set_round_cap_policy`).
//!
//! The test checks the recording holds what `play` replays from: every
//! engine call, the static answers, the runs with their calls, the
//! combines, the provider's answers and the absorbed batch.
//! `scripts/record-clean.sh` replays it with `play` and checks the replay
//! is the recording's.

#![cfg(feature = "observe")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use phitex_defuse::{
    CapPolicy, Ctx, Edit, EditQueue, Engine, Field, Greedy, Locality, NodeId, OpId, Operation, Sequential, Stop,
    SumMonoid, Value, ValueId, Values,
};
use serde_json::Value as J;

/// `["w", x]` contributes `x` to `pos`; `["p", j]` publishes loop-back
/// `lb{j}` = the fold of `pos` before it; `["r", j]` reads `lb{j}` and
/// contributes 2; `["e", name]` reads an environment field.
struct Toy;

fn text(v: &Values, x: Option<&ValueId>) -> String {
    x.map_or_else(String::new, |&x| match &*v.get(x) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    })
}

impl Operation for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let pos = v.field("pos");
        match text(v, a.first()).as_str() {
            "w" => {
                cx.contribute(pos, a[1]);
                Ok(ValueId::UNIT)
            }
            "p" => {
                let at = cx.acc_range(pos, None)?;
                cx.publish(v.field(&format!("lb{}", v.as_int(a[1]).unwrap())), at);
                Ok(ValueId::UNIT)
            }
            "e" => Ok(cx.use_env(v.field(&text(v, a.get(1))))?.unwrap_or(ValueId::NIL)),
            _ => {
                let got = cx.use_field(v.field(&format!("lb{}", v.as_int(a[1]).unwrap())))?;
                cx.contribute(pos, v.int(2));
                Ok(got.unwrap_or(ValueId::UNIT))
            }
        }
    }
}

/// The second engine's operation: `["leaf", x]` is `x`; `["chain", a, b]`
/// applies `leaf a` and `leaf b`, parking its stage at each stop, and
/// contributes 1 to `pos`; `["rd"]` folds `pos` before it, parking at the
/// stop; `["fa"]` places a continuation after itself through
/// `follow_after`; `["sc"]` contributes 1 to `pos` (5 when `big`) and says
/// so statically; `["env"]` reads environment field `eb`; `["mx", k]`
/// contributes `k` to `top`, whose fold is the greatest, and `["top"]`
/// folds it; `["nd", ""]` defines `who` as its node's id, `["who", k]`
/// reads it.
struct Pg {
    big: Arc<AtomicBool>,
}

impl Pg {
    fn amount(&self, v: &Values) -> ValueId {
        v.int(if self.big.load(Ordering::Relaxed) { 5 } else { 1 })
    }
}

impl Operation for Pg {
    fn name(&self) -> &'static str {
        "pg"
    }
    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if text(v, args.first()) == "nd" { vec![(v.field("who"), Locality::Local)] } else { Vec::new() })
    }
    fn static_contribs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (text(v, args.first()) == "sc").then(|| vec![(v.field("pos"), self.amount(v))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (me, pos) = (cx.op(), v.field("pos"));
        let leaf = |x: ValueId| vec![v.str("leaf"), x];
        match text(v, a.first()).as_str() {
            "leaf" => Ok(a[1]),
            "chain" => {
                let stage = cx.parked().and_then(|t| t.downcast::<u8>().ok()).map_or(0, |b| *b);
                if stage == 0 {
                    if let Err(s) = cx.apply(me, leaf(a[1])) {
                        cx.park(Box::new(0u8));
                        return Err(s);
                    }
                }
                match cx.apply(me, leaf(a[2])) {
                    Ok(y) => {
                        cx.contribute(pos, v.int(1));
                        Ok(y)
                    }
                    Err(s) => {
                        cx.park(Box::new(1u8));
                        Err(s)
                    }
                }
            }
            "rd" => {
                let _ = cx.parked();
                cx.acc_range(pos, None).inspect_err(|_| cx.park(Box::new(())))
            }
            "mx" => {
                cx.contribute(v.field("top"), a[1]);
                Ok(ValueId::UNIT)
            }
            "top" => cx.acc_range(v.field("top"), None),
            #[allow(clippy::cast_possible_wrap)]
            "nd" => {
                let who = v.int(cx.node().raw() as i64);
                cx.define(v.field("who"), who);
                Ok(ValueId::UNIT)
            }
            "who" => Ok(cx.use_field(v.field("who"))?.unwrap_or(ValueId::UNIT)),
            "fa" => {
                let me_node = cx.node();
                Ok(v.int(cx.follow_after(me_node, me, leaf(v.int(9))) as i64))
            }
            "sc" => {
                cx.contribute(pos, self.amount(v));
                Ok(ValueId::UNIT)
            }
            _ => Ok(cx.use_env(v.field("eb"))?.unwrap_or(ValueId::NIL)),
        }
    }
}

/// The greatest of the integers.
struct MaxMonoid;

impl phitex_defuse::Monoid for MaxMonoid {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(i64::MIN / 2)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap_or(0).max(v.as_int(b).unwrap_or(0)))
    }
}

/// The third engine's operation: publishes loop-back `lb` + 1 (never
/// settles).
struct Inc;

impl Operation for Inc {
    fn name(&self) -> &'static str {
        "inc"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let lb = v.field("lb");
        let x = cx.use_field(lb)?.and_then(|x| v.as_int(x)).unwrap_or(0);
        cx.publish(lb, v.int(x + 1));
        Ok(ValueId::UNIT)
    }
}

/// The text `[s, x]`.
fn t(eng: &Engine, s: &str, x: i64) -> Vec<ValueId> {
    vec![eng.values().str(s), eng.values().int(x)]
}

fn outputs(eng: &Engine, nodes: &[NodeId]) -> Vec<Option<ValueId>> {
    nodes.iter().map(|&n| eng.output(n)).collect()
}

#[test]
fn a_clean_recording_holds_what_play_replays() {
    let path = std::env::var_os("DEFUSE_RECORD").map_or_else(
        || {
            let p = std::env::temp_dir().join(format!("defuse-record-clean-{}.jsonl", std::process::id()));
            // (Before the first engine: the recorder opens its file once.)
            unsafe { std::env::set_var("DEFUSE_RECORD", &p) };
            p
        },
        std::path::PathBuf::from,
    );
    let mut eng = Engine::new();
    let toy = eng.register(Arc::new(Toy));
    let (int, field) = (|e: &Engine, x: i64| e.values().int(x), |e: &Engine, s: &str| e.values().field(s));
    eng.declare_accumulator(field(&eng, "pos"), Arc::new(SumMonoid));
    eng.declare_loopback(field(&eng, "lb0"), int(&eng, 0));
    eng.declare_loopbacks(&[(field(&eng, "lb1"), int(&eng, 0)), (field(&eng, "lb2"), int(&eng, -1))]);
    eng.set_environment([(field(&eng, "ea"), int(&eng, 5))]);
    let eb = field(&eng, "eb");
    eng.set_env_provider(Box::new(move |vals: &Values, f: Field| (f == eb).then(|| Some(vals.int(7)))));
    let u = eng.new_unit();
    let env = |e: &Engine, s: &str| vec![e.values().str("e"), e.values().str(s)];
    let mut nodes = vec![
        eng.push(u, toy, t(&eng, "r", 1)),
        eng.push(u, toy, t(&eng, "w", 3)),
        eng.push(u, toy, t(&eng, "p", 0)),
        eng.push(u, toy, t(&eng, "r", 0)),
        eng.push(u, toy, t(&eng, "w", 4)),
        eng.push(u, toy, t(&eng, "p", 1)),
        eng.push(u, toy, t(&eng, "r", 2)),
        eng.push(u, toy, t(&eng, "p", 2)),
        eng.push(u, toy, env(&eng, "ea")),
        eng.push(u, toy, env(&eng, "eb")),
    ];
    eng.add_root(nodes[0]);
    eng.mark(nodes[3]);
    let cold = eng.evaluate_sequential().expect("cold build");
    assert!(cold.rounds >= 2, "the loop-back fields take rounds: {cold:?}");
    let first = outputs(&eng, &nodes);
    assert_eq!(first[0], Some(int(&eng, 11)), "lb1: 2 + 3 + 2 + 4");
    assert_eq!(first[9], Some(int(&eng, 7)), "the provider's answer");

    // Edits between evaluations.
    eng.replace(nodes[1], toy, t(&eng, "w", 10));
    nodes.push(eng.insert_after(nodes[4], toy, t(&eng, "w", 1)));
    eng.remove(nodes[6]);
    eng.set_environment([(field(&eng, "ea"), int(&eng, 6))]);
    eng.set_version(2);
    eng.evaluate_sequential().expect("after the edits");
    let second = outputs(&eng, &nodes);
    assert_eq!(second[0], Some(int(&eng, 19)), "lb1: 2 + 10 + 2 + 4 + 1");
    assert_eq!(second[8], Some(int(&eng, 6)));

    // Edits absorbed during an evaluation.
    let q = EditQueue::new();
    q.push(
        Edit::Replace {
            node: nodes[4],
            op: toy,
            args: t(&eng, "w", 20),
        },
        3,
    );
    q.push(
        Edit::Push {
            unit: u,
            op: toy,
            args: t(&eng, "r", 1),
        },
        3,
    );
    eng.evaluate_absorbing(&mut Greedy::default(), &mut Sequential, &mut q.clone())
        .expect("absorbing");
    assert_eq!(eng.output(nodes[0]), Some(int(&eng, 35)), "lb1: 2 + 10 + 2 + 20 + 1");

    // A second engine in the same process.
    let big = Arc::new(AtomicBool::new(false));
    let known = Arc::new(AtomicBool::new(false));
    let mut eng2 = Engine::new();
    let pg = eng2.register(Arc::new(Pg { big: big.clone() }));
    eng2.declare_accumulator(field(&eng2, "pos"), Arc::new(SumMonoid));
    eng2.set_stable(field(&eng2, "pos"));
    eng2.set_heights(true);
    let k2 = known.clone();
    eng2.set_env_provider(Box::new(move |vals: &Values, f: Field| {
        (f == vals.field("eb") && k2.load(Ordering::Relaxed)).then(|| Some(vals.int(7)))
    }));
    let u2 = eng2.new_unit();
    let s = |e: &Engine, xs: &[&str]| xs.iter().map(|x| e.values().str(x)).collect::<Vec<_>>();
    let chain = vec![eng2.values().str("chain"), int(&eng2, 1), int(&eng2, 2)];
    let n2 = [
        eng2.push(u2, pg, chain),
        eng2.push(u2, pg, s(&eng2, &["rd"])),
        eng2.push(u2, pg, s(&eng2, &["fa"])),
        eng2.push(u2, pg, s(&eng2, &["sc"])),
        eng2.push(u2, pg, s(&eng2, &["rd"])),
        eng2.push(u2, pg, s(&eng2, &["env"])),
    ];
    // (Its own monoid: a fold the replay groups otherwise than the
    // recording is no recorded combine.)
    // (In a unit of their own: the environment read above waits.)
    eng2.declare_accumulator(field(&eng2, "top"), Arc::new(MaxMonoid));
    let u2b = eng2.new_unit();
    for k in [3, 9, 4] {
        eng2.push(u2b, pg, vec![eng2.values().str("mx"), int(&eng2, k)]);
    }
    let top = eng2.push(u2b, pg, s(&eng2, &["top"]));
    // Two nodes of one text whose runs differ by the node alone.
    let nd = [&["nd", ""], &["who", "1"], &["nd", ""], &["who", "2"]].map(|t| eng2.push(u2b, pg, s(&eng2, t)));
    let r = eng2.evaluate_sequential().expect("the second engine");
    assert!(r.resumed >= 3, "the chain resumes twice, the range read once: {r:?}");
    assert_eq!(eng2.output(n2[0]), Some(int(&eng2, 2)));
    assert_eq!(eng2.output(n2[1]), Some(int(&eng2, 1)));
    assert_eq!(eng2.output(n2[4]), Some(int(&eng2, 2)));
    assert_eq!(eng2.output(top), Some(int(&eng2, 9)));
    #[allow(clippy::cast_possible_wrap)]
    let id = |n: NodeId| Some(int(&eng2, n.raw() as i64));
    assert_eq!((eng2.output(nd[1]), eng2.output(nd[3])), (id(nd[0]), id(nd[2])));
    // The client's context changes: the provider knows `eb` now, and the
    // `sc` texts' hooks answer again.
    known.store(true, Ordering::Relaxed);
    big.store(true, Ordering::Relaxed);
    eng2.refresh_where(&|op: OpId, args: &[ValueId], v: &Values| op == pg && text(v, args.first()) == "sc");
    eng2.evaluate_sequential().expect("the second engine again");
    assert_eq!(eng2.output(n2[4]), Some(int(&eng2, 6)));
    assert_eq!(eng2.output(n2[5]), Some(int(&eng2, 7)));

    // A third: the round cap ends the rounds instead of failing.
    let mut eng3 = Engine::new();
    let inc = eng3.register(Arc::new(Inc));
    eng3.declare_loopback(field(&eng3, "lb"), int(&eng3, 0));
    eng3.set_max_rounds(3);
    eng3.set_round_cap_policy(CapPolicy::Finalize);
    let u3 = eng3.new_unit();
    eng3.push(u3, inc, Vec::new());
    let r = eng3.evaluate_sequential().expect("capped, not failed");
    assert!(r.capped.is_some(), "{r:?}");

    let text = std::fs::read_to_string(&path).expect("the recording");
    let all: Vec<J> = text.lines().map(|l| serde_json::from_str(l).expect("a JSON line")).collect();
    assert_eq!(all[0], serde_json::json!({"k": "rec", "v": 2}), "the format line first");
    let of = |e: &Engine| -> Vec<J> {
        let i = e.values().instance();
        all.iter().filter(|l| l["e"] == i).cloned().collect()
    };
    let (lines2, lines3) = (of(&eng2), of(&eng3));
    let api2 = |f: &str| lines2.iter().filter(|l| l["k"] == "api" && l["f"] == f).count();
    for f in ["set_stable", "set_heights", "refresh_where"] {
        assert_eq!(api2(f), 1, "no {f} line");
    }
    assert!(lines3.iter().any(|l| l["k"] == "api" && l["f"] == "set_round_cap_policy"));
    // Each `Ctx` call once: `follow_after` placing its continuation as
    // `follow`, `acc_range` reading as `acc_range_to`.
    let calls: Vec<&J> = lines2.iter().filter(|l| l["k"] == "run").flat_map(|l| l["calls"].as_array().unwrap()).collect();
    let called = |m: &str| calls.iter().filter(|c| c[0] == m).count();
    assert!(called("follow_after") >= 1 && called("acc_range") >= 1);
    assert_eq!((called("follow"), called("acc_range_to")), (0, 0));
    // The provider's answers for `eb`: unknown, then 7.
    let env: Vec<&J> = lines2.iter().filter(|l| l["k"] == "env").collect();
    assert!(env.first().is_some_and(|l| l["known"] == false) && env.last().is_some_and(|l| l["known"] == true));

    let lines = of(&eng);
    let api = |f: &str| lines.iter().filter(|l| l["k"] == "api" && l["f"] == f).count();
    let kind = |k: &str| lines.iter().filter(|l| l["k"] == k).count();
    // (The built-ins are the engine's own: only the client's op.)
    assert_eq!(api("register"), 1);
    assert_eq!(api("evaluate_begin"), 3);
    assert_eq!(api("evaluate_end"), 3);
    for f in [
        "declare_accumulator",
        "declare_loopback",
        "declare_loopbacks",
        "set_environment",
        "set_env_provider",
        "new_unit",
        "add_root",
        "mark",
        "replace",
        "insert_after",
        "remove",
        "set_version",
    ] {
        assert!(api(f) >= 1, "no {f} line");
    }
    // Ten pushes, the absorbed one among them.
    assert_eq!(api("push"), 11);
    // The source is asked at every step; the two queued edits come in one
    // batch, between its begin and end.
    let phase = |p: &str| lines.iter().filter(|l| l["k"] == "absorb" && l["phase"] == p).count();
    assert_eq!(phase("begin"), phase("end"));
    let (mut batches, mut open) = (Vec::new(), false);
    for l in &lines {
        if l["k"] == "absorb" {
            open = l["phase"] == "begin";
            if open {
                batches.push(Vec::new());
            }
        } else if l["k"] == "api" && open {
            batches.last_mut().expect("a begin").push(l["f"].clone());
        }
    }
    let edits: Vec<&Vec<J>> = batches.iter().filter(|b| !b.is_empty()).collect();
    assert_eq!(edits, [&vec![J::from("replace"), J::from("push")]]);
    for k in ["st", "run", "mono", "env", "val"] {
        assert!(kind(k) >= 1, "no {k} line");
    }
    // Each run's calls are there, and runs name the registered op.
    assert!(
        lines
            .iter()
            .filter(|l| l["k"] == "run")
            .all(|l| l["op"] == serde_json::json!(toy) && l["calls"].as_array().is_some_and(|c| !c.is_empty()))
    );
    eprintln!("recording: {} ({} lines)", path.display(), lines.len());
}
