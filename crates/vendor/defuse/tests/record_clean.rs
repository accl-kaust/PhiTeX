//! The clean core's `DEFUSE_RECORD` recording: a small program with
//! loop-back fields, an accumulator, the environment (supplied and from a
//! provider), edits between evaluations and edits absorbed during one.
//!
//! The test checks the recording holds what `play` replays from: every
//! engine call, the static answers, the runs with their calls, the
//! combines, the provider's answers and the absorbed batch.
//! `scripts/record-clean.sh` replays it with `play` and checks the replay
//! is the recording's.

#![cfg(all(feature = "clean", feature = "observe"))]

use std::sync::Arc;

use defuse::{
    Ctx, Edit, EditQueue, Engine, Field, Greedy, Locality, NodeId, Operation, Sequential, Stop, SumMonoid, Value,
    ValueId, Values,
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

    let text = std::fs::read_to_string(&path).expect("the recording");
    let lines: Vec<J> = text.lines().map(|l| serde_json::from_str(l).expect("a JSON line")).collect();
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
