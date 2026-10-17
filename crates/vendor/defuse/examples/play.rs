//! Replays a recording made by an `observe` build with `DEFUSE_RECORD=path`:
//! the engine calls in order, with playback operations, accumulators,
//! projections and an environment provider that answer from the file.
//! Nothing of the client is needed; a failing client run becomes an
//! engine-only repro.
//!
//! A playback run makes the recorded run's `Ctx` calls in order and checks
//! each answer against the recording; when an answer differs from every
//! recorded run of that node's text, the run is a divergence (counted and
//! printed, the run returns its best recorded result).
//!
//! `cargo run --release --features observe --example play -- REC [--engine E]
//!   [--threads T] [--spec off|predicted|all] [--budget B]`

#![allow(
    clippy::many_single_char_names,
    clippy::type_complexity,
    clippy::redundant_closure_for_method_calls,
    clippy::needless_pass_by_value,
    clippy::unit_arg,
    clippy::single_match_else,
    clippy::question_mark,
    clippy::struct_field_names,
    clippy::redundant_closure,
    clippy::collapsible_else_if,
    clippy::collapsible_if
)]

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use common::Threads;
use defuse::{
    Claim, Ctx, EditSource, Effect, Engine, Field, Greedy, Limits, Locality, LoopStart, MayDefine, Monoid, NodeId,
    OpId, Operation, ProjId, Speculation, Stop, UnitId, Until, Value, ValueId, Values,
};
use serde_json::{Value as J, json};

fn de<T: serde::de::DeserializeOwned>(j: &J) -> T {
    serde_json::from_value(j.clone()).unwrap_or_else(|e| panic!("bad recording field {j}: {e}"))
}

/// Everything the playback hooks answer from.
#[derive(Default)]
struct Book {
    /// (op, method, args, extra) -> answer.
    statics: HashMap<String, J>,
    /// (op, method, args) -> (items taken from `following`, answer).
    takes: HashMap<String, Vec<(Vec<(OpId, Vec<ValueId>)>, J)>>,
    /// (op, args) -> recorded runs: their calls and results.
    runs: HashMap<String, Vec<(Vec<J>, J)>>,
    /// (field, method, args) -> answer.
    monoid: HashMap<String, J>,
    /// (projection, value) -> answer.
    proj: HashMap<String, J>,
    /// field -> provider answers in order.
    env: HashMap<String, J>,
}

static DIVERGED: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

fn key(parts: &[&J]) -> String {
    parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("|")
}

struct PlayOp {
    book: Arc<Book>,
    id: OpId,
    name: &'static str,
}

impl PlayOp {
    fn st<T: serde::de::DeserializeOwned>(&self, m: &str, args: &[ValueId], x: J, dflt: T) -> T {
        match self
            .book
            .statics
            .get(&key(&[&json!(self.id), &json!(m), &json!(args), &x]))
        {
            Some(r) => de(r),
            None => {
                if MISSES.fetch_add(1, Ordering::Relaxed) < 20 {
                    eprintln!("play: no recorded {m} of {} {args:?} {x}", self.name);
                }
                dflt
            }
        }
    }

    fn take<T: serde::de::DeserializeOwned>(
        &self,
        m: &str,
        args: &[ValueId],
        it: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
        dflt: T,
    ) -> T {
        let Some(list) = self.book.takes.get(&key(&[&json!(self.id), &json!(m), &json!(args)])) else {
            if MISSES.fetch_add(1, Ordering::Relaxed) < 20 {
                eprintln!("play: no recorded {m} of {} {args:?}", self.name);
            }
            return dflt;
        };
        let mut got: Vec<(OpId, Vec<ValueId>)> = Vec::new();
        let mut done = false;
        // The shortest recorded take that matches what the iterator gives.
        let mut order: Vec<&(Vec<(OpId, Vec<ValueId>)>, J)> = list.iter().collect();
        order.sort_by_key(|(t, _)| t.len());
        for (took, r) in order {
            while got.len() < took.len() && !done {
                match it.next() {
                    Some(x) => got.push(x),
                    None => done = true,
                }
            }
            if got.len() >= took.len() && got[..took.len()] == took[..] {
                return de(r);
            }
        }
        MISSES.fetch_add(1, Ordering::Relaxed);
        dflt
    }
}

/// Makes one recorded call on `cx`; returns its answer as recorded, or
/// `Err` with the run's stop when the call stopped it.
#[allow(clippy::too_many_lines)]
fn call(cx: &mut Ctx<'_>, m: &str, a: &J) -> Result<J, Stop> {
    fn res<T: serde::Serialize>(r: Result<T, Stop>) -> Result<J, Stop> {
        r.map(|x| json!({"ok": x}))
    }
    let g = |i: usize| a.get(i).cloned().unwrap_or(J::Null);
    Ok(match m {
        "use_field" => return res(cx.use_field(de(&g(0)))),
        "use_field_at" => return res(cx.use_field_at(de(&g(0)), de::<Option<NodeId>>(&g(1)))),
        "use_field_proj" => return res(cx.use_field_proj(de(&g(0)), de::<ProjId>(&g(1)))),
        "use_env" => return res(cx.use_env(de(&g(0)))),
        "define" => json!(cx.define(de(&g(0)), de(&g(1)))),
        "define_with" => json!(cx.define_with(de(&g(0)), de(&g(1)), de::<Locality>(&g(2)))),
        "publish" => json!(cx.publish(de(&g(0)), de(&g(1)))),
        "declare_loopback" => json!(cx.declare_loopback(de(&g(0)), de(&g(1)))),
        "child" => json!(cx.child(de(&g(0)), de(&g(1)))),
        "follow" => json!(cx.follow(de(&g(0)), de(&g(1)))),
        "open_scope" => json!(cx.open_scope()),
        "close_scope" => json!(cx.close_scope()),
        "output_of" => return res(cx.output_of(de(&g(0)))),
        "apply" => return res(cx.apply(de(&g(0)), de(&g(1)))),
        "contribute" => json!(cx.contribute(de(&g(0)), de(&g(1)))),
        "acc_range" => return res(cx.acc_range(de(&g(0)), de::<Option<NodeId>>(&g(1)))),
        "acc_range_to" => return res(cx.acc_range_to(de(&g(0)), de(&g(1)), de(&g(2)))),
        "scope_kind" => return res(cx.scope_kind()),
        "set_kind" => json!(cx.set_kind(de(&g(0)))),
        "set_kinds" => json!(cx.set_kinds(&de::<Vec<ValueId>>(&g(0)))),
        "set_effect" => json!(cx.set_effect(de::<Effect>(&g(0)))),
        "effect_pending" => json!(cx.effect_pending()),
        "narrow" => json!(cx.narrow(de::<MayDefine>(&g(0)))),
        "claim" => json!(cx.claim(de(&g(0)))),
        "claimed" => json!(cx.claimed()),
        "following" => json!(cx.following(de(&g(0)))),
        "claim_stream" => return res(cx.claim_stream()),
        "prev_sibling_where" => {
            let asked: Vec<(OpId, Vec<ValueId>, bool)> = de(a);
            json!(
                cx.prev_sibling_where(&|op, args| { asked.iter().any(|(o, xs, b)| *b && *o == op && xs[..] == *args) })
            )
        }
        "push_contribution" => json!(cx.push_contribution(de(&g(0)), de(&g(1)))),
        "prev_contribution" => return res(cx.prev_contribution(de(&g(0)))),
        "contributions_before_me" => return res(cx.contributions_before_me(de(&g(0)))),
        "take_prev_contribution" => return res(cx.take_prev_contribution(de(&g(0)), de(&g(1)))),
        "set_env" => json!(cx.set_env(de(&g(0)), de(&g(1)))),
        "park" => json!(cx.park(Box::new(()))),
        "parked" => json!(cx.parked().is_some()),
        "restart" => json!(cx.restart()),
        "diagnostic" => json!(cx.diagnostic(de(&g(0)))),
        "is_loopback" => json!(cx.is_loopback(de(&g(0)))),
        other => panic!("play: unknown Ctx call {other}"),
    })
}

impl Operation for PlayOp {
    fn name(&self) -> &'static str {
        self.name
    }
    fn defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        self.st("defs", args, J::Null, None)
    }
    fn may_define(&self, args: &[ValueId], _: &Values) -> MayDefine {
        self.st("may_define", args, J::Null, MayDefine::Any)
    }
    fn may_define_field(&self, args: &[ValueId], _: &Values, f: Field) -> bool {
        self.st("may_define_field", args, json!(f), true)
    }
    fn reads(&self, args: &[ValueId], _: &Values) -> Option<Vec<Field>> {
        self.st("reads", args, J::Null, None)
    }
    fn static_contribs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        self.st("static_contribs", args, J::Null, None)
    }
    fn predicted_defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        self.st("predicted_defs", args, J::Null, None)
    }
    fn effect(&self, args: &[ValueId], _: &Values) -> Effect {
        self.st("effect", args, J::Null, Effect::None)
    }
    fn claim(&self, args: &[ValueId], _: &Values, it: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        self.take("claim", args, it, Claim::None)
    }
    fn claim_limit(
        &self,
        args: &[ValueId],
        _: &Values,
        it: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        self.take("claim_limit", args, it, None)
    }
    fn until(&self, args: &[ValueId], _: &Values, cand: OpId, cand_args: &[ValueId]) -> Until {
        self.st("until", args, json!([cand, cand_args]), Until::No)
    }
    fn until_value(&self, args: &[ValueId], _: &Values, cand: OpId, cand_args: &[ValueId], v: Option<ValueId>) -> bool {
        self.st("until_value", args, json!([cand, cand_args, v]), v.is_some())
    }
    fn until_values(
        &self,
        args: &[ValueId],
        _: &Values,
        cand: OpId,
        cand_args: &[ValueId],
        fields: &[Option<ValueId>],
    ) -> bool {
        self.st(
            "until_values",
            args,
            json!([cand, cand_args, fields]),
            fields.iter().all(Option::is_some),
        )
    }
    fn until_predict(
        &self,
        args: &[ValueId],
        _: &Values,
        it: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        self.take("until_predict", args, it, None)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let k = key(&[&json!(self.id), &json!(cx.args())]);
        let Some(runs) = self.book.runs.get(&k) else {
            DIVERGED.fetch_add(1, Ordering::Relaxed);
            eprintln!("play: no recorded run of {} {:?}", self.name, cx.args());
            return Ok(ValueId::UNIT);
        };
        let mut live: Vec<usize> = (0..runs.len()).collect();
        let mut i = 0;
        loop {
            // A candidate that ends here returns its result.
            if let Some(&c) = live.iter().find(|&&c| runs[c].0.len() == i) {
                return match &runs[c].1 {
                    J::Object(o) => Ok(de(&o["ok"])),
                    _ => {
                        // Recorded as stopped without a stopping call here:
                        // the engine stopped it on a later check; replay
                        // the same through a read that cannot answer.
                        Ok(ValueId::UNIT)
                    }
                };
            }
            let c0 = &runs[live[0]].0[i];
            let (m, a) = (c0[0].as_str().unwrap_or(""), c0[1].clone());
            let got = call(cx, m, &a);
            let ans = match &got {
                Ok(j) => j.clone(),
                Err(_) => json!("stop"),
            };
            live.retain(|&c| {
                runs[c].0.len() > i && runs[c].0[i][0] == c0[0] && (runs[c].0[i][2] == ans || runs[c].0[i][2] == "*")
            });
            if let Err(stop) = got {
                return Err(stop);
            }
            if live.is_empty() {
                DIVERGED.fetch_add(1, Ordering::Relaxed);
                let want: Vec<String> = runs
                    .iter()
                    .filter(|r| r.0.len() > i)
                    .map(|r| r.0[i][2].to_string())
                    .collect();
                eprintln!(
                    "play: {} {:?} diverged at call {i} ({m} {a}): got {ans}, recorded {}",
                    self.name,
                    cx.args(),
                    want.join(" | ")
                );
                return Ok(ValueId::UNIT);
            }
            i += 1;
        }
    }
}

struct PlayMonoid {
    book: Arc<Book>,
    field: J,
}

impl Monoid for PlayMonoid {
    fn empty(&self, _: &Values) -> ValueId {
        self.book
            .monoid
            .get(&key(&[&self.field, &json!("empty"), &J::Null]))
            .map_or(ValueId::NIL, de)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        if let Some(r) = self
            .book
            .monoid
            .get(&key(&[&self.field, &json!("combine"), &json!([a, b])]))
        {
            return de(r);
        }
        // Not recorded (the engine grouped the fold another way): the
        // recorded accumulators are sequences, sums and list concatenations.
        match (&*v.get(a), &*v.get(b)) {
            (Value::Int(x), Value::Int(y)) => v.int(x + y),
            (Value::List(x), Value::List(y)) => v.list(x.iter().chain(y.iter()).copied().collect()),
            _ => v.seq_concat(a, b),
        }
    }
}

/// Edits recorded inside one evaluation, absorbed in their batches.
struct Batches {
    batches: std::collections::VecDeque<Vec<J>>,
    book: Arc<Book>,
    units: Arc<Mutex<Vec<UnitId>>>,
}

impl EditSource for Batches {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        let b = self.batches.pop_front()?;
        let mut v = None;
        for l in &b {
            if l["k"] == "absorb" {
                v = de::<Option<u64>>(&l["r"]);
            } else {
                apply(eng, l, &self.book, &self.units);
            }
        }
        v
    }
}

/// Applies one recorded engine call.
fn apply(eng: &mut Engine, l: &J, book: &Arc<Book>, units: &Arc<Mutex<Vec<UnitId>>>) {
    let a = &l["a"];
    let g = |i: usize| a.get(i).cloned().unwrap_or(J::Null);
    let want = &l["r"];
    let f = l["f"].as_str().unwrap_or("");
    let check = |what: &str, got: J| {
        if got != *want {
            eprintln!("play: {what} returned {got}, recorded {want}");
        }
    };
    match f {
        "register" => {
            let name: &'static str = Box::leak(a[0].as_str().unwrap_or("op").to_string().into_boxed_str());
            let id: OpId = de(want);
            let got = eng.register(Arc::new(PlayOp {
                book: book.clone(),
                id,
                name,
            }));
            check("register", json!(got));
        }
        "register_projection" => {
            let id = want.as_u64().unwrap_or(0);
            let b = book.clone();
            eng.register_projection(Arc::new(move |v: ValueId, _: &Values| {
                b.proj.get(&key(&[&json!(id), &json!(v)])).map_or(v, de)
            }));
        }
        "declare_accumulator" => {
            eng.declare_accumulator(
                de(a),
                Arc::new(PlayMonoid {
                    book: book.clone(),
                    field: a.clone(),
                }),
            );
        }
        "set_env_provider" => {
            let b = book.clone();
            eng.set_env_provider(Box::new(move |_: &Values, f: Field| {
                b.env.get(&json!(f).to_string()).and_then(|r| de(r))
            }));
        }
        "new_unit" => {
            let u = eng.new_unit();
            units.lock().unwrap().push(u);
            check("new_unit", json!(u));
        }
        "push" => check("push", json!(eng.push(de(&g(0)), de(&g(1)), de(&g(2))))),
        "add_root" => eng.add_root(de(&g(0))),
        "insert_after" => check("insert_after", json!(eng.insert_after(de(&g(0)), de(&g(1)), de(&g(2))))),
        "insert_before" => check(
            "insert_before",
            json!(eng.insert_before(de(&g(0)), de(&g(1)), de(&g(2)))),
        ),
        "replace" => eng.replace(de(&g(0)), de(&g(1)), de(&g(2))),
        "remove" => eng.remove(de(&g(0))),
        "declare_sequence" => eng.declare_sequence(de(&g(0))),
        "declare_loopback" => eng.declare_loopback(de(&g(0)), de(&g(1))),
        "set_loop_start" => eng.set_loop_start(de::<LoopStart>(&g(0))),
        "set_max_rounds" => eng.set_max_rounds(de(&g(0))),
        "set_limits" => eng.set_limits(de::<Limits>(&g(0))),
        "set_environment" => eng.set_environment(de::<Vec<(Field, ValueId)>>(a)),
        "set_environment_absent" => eng.set_environment_absent(de::<Vec<Field>>(a)),
        "set_env" => eng.set_env(de(&g(0)), de(&g(1)), de(&g(2))),
        "clear_env_provider" => eng.clear_env_provider(),
        "remove_root" => eng.remove_root(de(&g(0))),
        "mark_op" => eng.mark_op(de(&g(0))),
        "collect_garbage" => {
            let _ = eng.collect_garbage();
        }
        "mark" => eng.mark(de(&g(0))),
        "unmark" => eng.unmark(de(&g(0))),
        "set_version" => eng.set_version(de(&g(0))),
        "pin" | "unpin" | "evaluate_begin" | "evaluate_end" => {}
        other => eprintln!("play: skipped engine call {other}"),
    }
}

fn intern(vals: &Values, l: &J) -> ValueId {
    let x = &l["x"];
    let ids = |x: &J| -> Vec<ValueId> { de(x) };
    let got = match l["t"].as_str().unwrap_or("") {
        "unit" => ValueId::UNIT,
        "nil" => ValueId::NIL,
        "bool" => vals.bool(x.as_bool().unwrap_or(false)),
        "int" => vals.int(x.as_i64().unwrap_or(0)),
        "str" => vals.str(x.as_str().unwrap_or("")),
        "list" => vals.intern(Value::List(ids(x).into_boxed_slice())),
        "seq" => ids(x)
            .into_iter()
            .fold(ValueId::NIL, |acc, e| vals.seq_concat(acc, vals.seq_unit(e))),
        t => panic!("play: unknown value type {t}"),
    };
    let want: ValueId = de(&l["id"]);
    assert_eq!(got, want, "play: value {l} interned to a different id");
    got
}

#[allow(clippy::too_many_lines)]
fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let path = argv
        .get(1)
        .expect("usage: play REC [--engine E] [--threads T] [--spec off|predicted|all] [--budget B]");
    let opt = |name: &str| {
        argv.iter()
            .position(|a| a == name)
            .and_then(|i| argv.get(i + 1))
            .cloned()
    };
    let threads: usize = opt("--threads").map_or(1, |s| s.parse().unwrap());
    let budget: usize = opt("--budget").map_or(64, |s| s.parse().unwrap());
    let spec = match opt("--spec").as_deref() {
        Some("all") => Speculation::All,
        Some("predicted") => Speculation::Predicted,
        _ => Speculation::Off,
    };
    let text = std::fs::read_to_string(path).expect("read recording");
    let lines: Vec<J> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    // The engine: the one asked for, else the first that evaluated.
    let engine = opt("--engine").map(|s| s.parse::<u64>().unwrap()).or_else(|| {
        lines
            .iter()
            .find(|l| l["k"] == "api" && l["f"] == "evaluate_begin")
            .and_then(|l| l["e"].as_u64())
    });
    let Some(engine) = engine else {
        eprintln!("play: the recording has no evaluation");
        std::process::exit(2);
    };
    let lines: Vec<J> = lines.into_iter().filter(|l| l["e"].as_u64() == Some(engine)).collect();
    let mut book = Book::default();
    for l in &lines {
        match l["k"].as_str().unwrap_or("") {
            "st" => {
                let m = &l["m"];
                if matches!(m.as_str(), Some("claim" | "claim_limit" | "until_predict")) {
                    book.takes
                        .entry(key(&[&l["op"], m, &l["a"]]))
                        .or_default()
                        .push((de(&l["x"]), l["r"].clone()));
                } else {
                    book.statics
                        .insert(key(&[&l["op"], m, &l["a"], &l["x"]]), l["r"].clone());
                }
            }
            "run" => {
                let calls = l["calls"].as_array().cloned().unwrap_or_default();
                book.runs
                    .entry(key(&[&l["op"], &l["a"]]))
                    .or_default()
                    .push((calls, l["r"].clone()));
            }
            "mono" => {
                let a = if l["m"] == "empty" { J::Null } else { l["a"].clone() };
                book.monoid.insert(key(&[&l["f"], &l["m"], &a]), l["r"].clone());
            }
            "proj" => {
                book.proj.insert(key(&[&l["p"], &l["a"]]), l["r"].clone());
            }
            "env" => {
                book.env.insert(l["f"].to_string(), l["r"].clone());
            }
            _ => {}
        }
    }
    for runs in book.runs.values_mut() {
        // A resumed run (its first call answered `parked` true) is recorded
        // from where it stopped; when the replay restarts it instead, the
        // fresh run is the stopped run's calls up to its blocked read
        // (answered now: any answer) followed by the resumed calls.
        let mut fresh = Vec::new();
        let mut stopped: Option<&Vec<J>> = None;
        for (calls, r) in runs.iter() {
            let resumed = calls.first().is_some_and(|c| c[0] == "parked" && c[2] == true);
            if let (true, Some(prev)) = (resumed, stopped) {
                if prev.len() >= 2 && prev[prev.len() - 1][0] == "park" {
                    let mut c: Vec<J> = prev[..prev.len() - 1].to_vec();
                    if let Some(last) = c.last_mut() {
                        last[2] = json!("*");
                    }
                    c.extend(calls[1..].iter().cloned());
                    fresh.push((c, r.clone()));
                }
            }
            stopped = (*r == json!("stop")).then_some(calls);
        }
        runs.extend(fresh);
        runs.dedup();
    }
    let book = Arc::new(book);
    let units = Arc::new(Mutex::new(Vec::new()));
    let mut eng = Engine::new();
    let mut evals = 0;
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        i += 1;
        match l["k"].as_str().unwrap_or("") {
            "val" => {
                let v = intern(eng.values(), l);
                eng.pin(v);
            }
            "api" if l["f"] == "evaluate_begin" => {
                // Gather this evaluation's absorbed batches.
                let mut batches = std::collections::VecDeque::new();
                let mut cur: Option<Vec<J>> = None;
                let mut want = J::Null;
                while i < lines.len() {
                    let l = &lines[i];
                    i += 1;
                    if l["k"] == "val" {
                        let v = intern(eng.values(), l);
                        eng.pin(v);
                    } else if l["k"] == "absorb" && l["phase"] == "begin" {
                        cur = Some(Vec::new());
                    } else if l["k"] == "absorb" && l["phase"] == "end" {
                        let mut b = cur.take().unwrap_or_default();
                        b.push(l.clone());
                        batches.push_back(b);
                    } else if l["k"] == "api" && l["f"] == "evaluate_end" {
                        want = l["r"].clone();
                        break;
                    } else if l["k"] == "api" {
                        if let Some(c) = cur.as_mut() {
                            c.push(l.clone());
                        }
                    }
                }
                evals += 1;
                let mut src = Batches {
                    batches,
                    book: book.clone(),
                    units: units.clone(),
                };
                let mut sched = Greedy::speculative(spec, budget);
                eprintln!("play: evaluating {}", evals);
                std::thread::sleep(std::time::Duration::from_millis(
                    std::env::var("PLAY_PAUSE_MS")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                ));
                let t0 = std::time::Instant::now();
                let r = eng.evaluate_absorbing(&mut sched, &mut Threads(threads), &mut src);
                let got = match &r {
                    Ok(rep) => json!({"ok": {"nodes_run": rep.nodes_run, "rounds": rep.rounds}}),
                    Err(e) => json!(e.to_string()),
                };
                let same = if got == want {
                    "same as recorded"
                } else {
                    "DIFFERENT from recorded"
                };
                println!("evaluation {evals}: {:.1?}, {}: {same}", t0.elapsed(), short(&got));
                if got != want {
                    println!("  recorded: {}", short(&want));
                }
                // Committed reads that no longer hold (none in a sound state).
                let stale = eng.check_reads();
                println!("  stale reads {}", stale.len());
                for l in stale.iter().take(10) {
                    println!("    {l}");
                }
                if let Ok(rep) = &r {
                    println!(
                        "  runs {} stops {} rounds {} steps {}",
                        rep.nodes_run, rep.stops, rep.rounds, rep.steps
                    );
                    println!("  restarted_by {:?} resumed {}", rep.restarted_by, rep.resumed);
                }
            }
            "api" => apply(&mut eng, l, &book, &units),
            _ => {}
        }
    }
    println!(
        "replayed {evals} evaluations; {} divergent runs, {} unrecorded hook answers",
        DIVERGED.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed)
    );
}

fn short(j: &J) -> String {
    let s = j.to_string();
    if s.len() > 300 { format!("{}…", &s[..300]) } else { s }
}
