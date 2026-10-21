//! Replays a recording made by an `observe` build with `DEFUSE_RECORD=path`:
//! the engine calls in order, with playback operations, accumulators,
//! projections and an environment provider that answer from the file.
//! Nothing of the client is needed; a failing client run becomes an
//! engine-only repro.
//!
//! A playback run makes the recorded run's `Ctx` calls in order and checks
//! each answer against the recording; when an answer differs from every
//! recorded run of that node's text, the run is a divergence (counted and
//! printed, the run goes on as the recorded run it followed). Of a text's
//! recorded runs, the node's own and the latest recorded are followed
//! first. A run the engine stops makes the calls the recorded run made
//! after its stop (a `park`) and ends as it ended.
//!
//! A hook or the provider answers as it last answered at that point of the
//! recording (a client's answers may change between evaluations).
//!
//! An accumulator's folds the engine groups otherwise than the recording
//! did are formal where the monoid is the client's own (see
//! [`PlayMonoid`]).
//!
//! A process may record several engines (a format built in an engine of
//! its own before the document's): each is replayed in turn, in a fresh
//! engine, or only engine `E` (`--engine E`, the instance in the lines'
//! `e`).
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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::Threads;
use defuse::{
    CapPolicy, Claim, Ctx, EditSource, Effect, Engine, Field, Greedy, Limits, Locality, LoopStart, MayDefine, Monoid,
    NodeId, OpId, Operation, ProjId, Speculation, Stop, UnitId, Until, Value, ValueId, Values,
};
use serde_json::{Value as J, json};

fn de<T: serde::de::DeserializeOwned>(j: &J) -> T {
    serde_json::from_value(j.clone()).unwrap_or_else(|e| panic!("bad recording field {j}: {e}"))
}

/// Answers by the index of the line that recorded them, in order.
type Timed = Vec<(usize, J)>;

/// Everything the playback hooks answer from.
#[derive(Default)]
struct Book {
    /// (op, method, args, extra) -> answers.
    statics: HashMap<String, Timed>,
    /// (op, method, args) -> (line, items taken from `following`, answer).
    takes: HashMap<String, Vec<(usize, Vec<(OpId, Vec<ValueId>)>, J)>>,
    /// (op, args) -> recorded runs.
    runs: HashMap<String, Vec<Run>>,
    /// (field, method, args) -> answer.
    monoid: HashMap<String, J>,
    /// The accumulators (fields) whose recorded combines are all a sum, a
    /// list or a sequence concatenation: an unrecorded combine is that.
    exact: std::collections::HashSet<String>,
    /// The formal folds made (see [`PlayMonoid`]), and what each is when
    /// known.
    formal: Mutex<HashMap<u64, Option<u64>>>,
    /// (projection, value) -> answer.
    proj: HashMap<String, J>,
    /// field -> the provider's answers (their lines).
    env: HashMap<String, Timed>,
    /// Every string value recorded (the fields a prefix declaration names).
    strs: Vec<String>,
}

static DIVERGED: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Where the replay is in the recording: the line of the engine call being
/// made, or of the evaluation's end while it evaluates.
static AT: AtomicUsize = AtomicUsize::new(0);

/// The answer last recorded at or before [`AT`] (else the first).
fn now(list: &Timed) -> &J {
    let k = list.partition_point(|(i, _)| *i <= AT.load(Ordering::Relaxed));
    &list[k.saturating_sub(1)].1
}

fn miss(what: std::fmt::Arguments<'_>) {
    if MISSES.fetch_add(1, Ordering::Relaxed) < 20 {
        eprintln!("play: no recorded {what}");
    }
}

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
        self.st_inner(m, args, x, dflt)
    }
    fn st_inner<T: serde::de::DeserializeOwned>(&self, m: &str, args: &[ValueId], x: J, dflt: T) -> T {
        match self
            .book
            .statics
            .get(&key(&[&json!(self.id), &json!(m), &json!(args), &x]))
        {
            Some(r) => de(now(r)),
            None => {
                miss(format_args!("{m} of {} {args:?} {x}", self.name));
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
            miss(format_args!("{m} of {} {args:?}", self.name));
            return dflt;
        };
        let mut got: Vec<(OpId, Vec<ValueId>)> = Vec::new();
        let mut done = false;
        // A recorded take that matches what the iterator gives: of those
        // recorded so far the latest, then the shortest.
        let at = AT.load(Ordering::Relaxed);
        let mut order: Vec<&(usize, Vec<(OpId, Vec<ValueId>)>, J)> = list.iter().collect();
        order.sort_by_key(|(i, t, _)| (*i > at, if *i > at { 0 } else { usize::MAX - i }, t.len()));
        for (_, took, r) in order {
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
        miss(format_args!("{m} of {} {args:?} taking {got:?}", self.name));
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
        "use_field_before" => return res(cx.use_field_before(de(&g(0)), de::<NodeId>(&g(1)))),
        "use_field_after_own" => return res(cx.use_field_after_own(de(&g(0)), de::<NodeId>(&g(1)))),
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
        "acc_range_after_def" => return res(cx.acc_range_after_def(de(&g(0)), de(&g(1)))),
        "acc_range_to" => return res(cx.acc_range_to(de(&g(0)), de(&g(1)), de(&g(2)))),
        "acc_fold" => return res(cx.acc_fold(de(&g(0)), de(&g(1)), de(&g(2)))),
        "acc_next" => return res(cx.acc_next(de(&g(0)), de(&g(1)))),
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
        "follow_after" => json!(cx.follow_after(de(&g(0)), de(&g(1)), de(&g(2)))),
        "follow_after_any" => json!(cx.follow_after_any(de(&g(0)), de(&g(1)), de(&g(2)))),
        "park" => json!(cx.park(Box::new(Vec::<usize>::new()))),
        "park_continuing" => json!(cx.park_continuing(Box::new(Vec::<usize>::new()))),
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
    fn reads(&self, args: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        // `PLAY_DROP_READS=a,b`: those fields left out of the recorded
        // hints (a client that no longer declares them).
        let r: Option<Vec<Field>> = self.st("reads", args, J::Null, None);
        match std::env::var("PLAY_DROP_READS") {
            Ok(drop) => r.map(|fs| {
                fs.into_iter()
                    .filter(|f| v.as_str(f.0).is_none_or(|n| !drop.split(',').any(|d| d == n)))
                    .collect()
            }),
            Err(_) => r,
        }
    }
    fn static_contribs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        self.st("static_contribs", args, J::Null, None)
    }
    fn predicted_defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        self.st("predicted_defs", args, J::Null, None)
    }
    fn contribs_complete(&self, args: &[ValueId], _: &Values) -> bool {
        self.st("contribs_complete", args, J::Null, false)
    }
    fn final_only(&self, args: &[ValueId], _: &Values) -> bool {
        self.st("final_only", args, J::Null, false)
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
        self.run_inner(cx)
    }
}
impl PlayOp {
    fn run_inner(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let k = key(&[&json!(self.id), &json!(cx.args())]);
        let Some(runs) = self.book.runs.get(&k) else {
            DIVERGED.fetch_add(1, Ordering::Relaxed);
            eprintln!("play: no recorded run of {} {:?}", self.name, cx.args());
            return Ok(ValueId::UNIT);
        };
        // The candidates, the latest recorded by this point first: a call
        // whose answer picks none of them (a definition, say) is made as
        // the client last made it.
        let at = AT.load(Ordering::Relaxed);
        // Those of this node first: its runs may differ from another
        // node's of the same text where no answer tells them apart (a
        // value made of the node's id, say).
        let me = json!(cx.node());
        let mut live: Vec<usize> = (0..runs.len()).collect();
        live.sort_by_key(|&c| {
            let o = &runs[c].origins;
            let mine = usize::from(!runs[c].nodes.contains(&me));
            match o.partition_point(|&x| x <= at) {
                0 => (mine, 1, o[0]),
                k => (mine, 0, usize::MAX - o[k - 1]),
            }
        });
        let mut i = 0;
        loop {
            // A candidate that ends here returns its result.
            if let Some(&c) = live.iter().find(|&&c| runs[c].calls.len() == i) {
                return match &runs[c].r {
                    J::Object(o) => Ok(de(&o["ok"])),
                    _ => {
                        // Recorded as stopped without a stopping call here:
                        // the engine stopped it on a later check; replay
                        // the same through a read that cannot answer.
                        Ok(ValueId::UNIT)
                    }
                };
            }
            let followed = live[0];
            let c0 = &runs[followed].calls[i];
            let (m, a) = (c0[0].as_str().unwrap_or(""), c0[1].clone());
            let got = match m {
                // The token parked names the recorded runs that parked it:
                // the run goes on as one that resumed them.
                "parked" => {
                    let t = cx.parked();
                    let ans = json!(t.is_some());
                    if let Some(Ok(by)) = t.map(|t| t.downcast::<Vec<usize>>()) {
                        let next: Vec<usize> = live
                            .iter()
                            .copied()
                            .filter(|&c| runs[c].resumes.iter().any(|r| by.contains(r)))
                            .collect();
                        if !next.is_empty() {
                            live = next;
                        }
                    }
                    Ok(ans)
                }
                _ => runs[live[0]].call(cx, i),
            };
            let ans = match &got {
                Ok(j) => j.clone(),
                Err(_) => json!("stop"),
            };
            let vals = cx.values();
            live.retain(|&c| {
                let r = &runs[c].calls;
                r.len() > i && r[i][0] == c0[0] && (r[i][2] == "*" || self.book.same(vals, &ans, &r[i][2]))
            });
            if let Some(&c) = live.first() {
                self.book.learn(vals, &ans, &runs[c].calls[i][2]);
            }
            if let Err(stop) = got {
                // The calls the client made after the stop (it parks its
                // state, say), and its result, as a run that stopped here
                // recorded them.
                if let Some(&c) = live.iter().find(|&&c| runs[c].calls[i][2] == "stop") {
                    for j in i + 1..runs[c].calls.len() {
                        let _ = runs[c].call(cx, j);
                    }
                    if let J::Object(o) = &runs[c].r {
                        return Ok(de(&o["ok"]));
                    }
                }
                return Err(stop);
            }
            if live.is_empty() {
                DIVERGED.fetch_add(1, Ordering::Relaxed);
                let want: Vec<String> = runs
                    .iter()
                    .filter(|r| r.calls.len() > i)
                    .map(|r| r.calls[i][2].to_string())
                    .collect();
                eprintln!(
                    "play: {} {:?} diverged at call {i} ({m} {a}): got {ans}, recorded {}",
                    self.name,
                    cx.args(),
                    want.join(" | ")
                );
                // The run goes on as the recorded run it followed (its
                // children and definitions made: what comes after does not
                // diverge for want of them).
                for j in i + 1..runs[followed].calls.len() {
                    runs[followed].call(cx, j)?;
                }
                return Ok(match &runs[followed].r {
                    J::Object(o) => de(&o["ok"]),
                    _ => ValueId::UNIT,
                });
            }
            i += 1;
        }
    }
}

/// A recorded run of a text (identical runs merged).
#[derive(Default)]
struct Run {
    /// Its `Ctx` calls (method, arguments, answer) and its result.
    calls: Vec<J>,
    r: J,
    /// The lines of the runs it stands for, and of the stopped runs they
    /// resumed.
    origins: Vec<usize>,
    resumes: Vec<usize>,
    /// The nodes that ran it.
    nodes: Vec<J>,
}

impl Run {
    /// Makes call `i` on `cx`; a park keeps the runs this one stands for.
    fn call(&self, cx: &mut Ctx<'_>, i: usize) -> Result<J, Stop> {
        let c = &self.calls[i];
        match c[0].as_str().unwrap_or("") {
            "park" => {
                cx.park(Box::new(self.origins.clone()));
                Ok(J::Null)
            }
            "park_continuing" => {
                cx.park_continuing(Box::new(self.origins.clone()));
                Ok(J::Null)
            }
            m => call(cx, m, &c[1]),
        }
    }
}

/// An accumulator in playback. The engine folds a range in whatever
/// association its tree has then, which need not be the recording's: a
/// combine the recording never made is asked for. Where the recorded
/// combines are all a sum, a list or a sequence concatenation, that is
/// what an unrecorded one is. Otherwise (a client's own monoid) a fold is
/// formal: the sequence of the contributions folded, each `[MARK, field,
/// x]` (one contribution is itself, none the empty value). Sequence ids
/// are associative, so equal folds have equal ids whatever the grouping;
/// an op handed one is answered with the value the recorded run got (or
/// the fold of the recorded combines, when they make it).
struct PlayMonoid {
    book: Arc<Book>,
    field: J,
    exact: bool,
    /// The client monoid's `speculate` (its declaration's answer).
    speculate: bool,
    /// The recorded empty value.
    zero: Option<ValueId>,
}

/// The first element of a formal fold's contributions.
const MARK: &str = "\u{1}play:fold";

/// The contributions of formal fold `v` (`None`: not one).
fn formal_leaves(v: &Values, x: ValueId) -> Option<(J, Vec<ValueId>)> {
    let Value::Seq(n) = &*v.get(x) else { return None };
    let Value::List(e) = &*v.get(n.elem) else { return None };
    if e.len() != 3 || v.as_str(e[0]).is_none_or(|m| m != MARK) {
        return None;
    }
    let field = json!(e[1]);
    let leaves = v
        .seq_to_vec(x)
        .into_iter()
        .filter_map(|w| match &*v.get(w) {
            Value::List(e) if e.len() == 3 => Some(e[2]),
            _ => None,
        })
        .collect();
    Some((field, leaves))
}

impl PlayMonoid {
    fn zero(&self) -> Option<ValueId> {
        self.zero
    }
    /// `x` as a sequence of marked contributions.
    fn marked(&self, v: &Values, x: ValueId) -> ValueId {
        if Some(x) == self.zero() || x == ValueId::NIL && self.zero().is_none() {
            return ValueId::NIL;
        }
        if self.book.formal.lock().unwrap().contains_key(&x.raw()) {
            return x;
        }
        let f: ValueId = de(&self.field);
        v.seq_unit(v.list(vec![v.str(MARK), f, x]))
    }
}

impl Monoid for PlayMonoid {
    fn speculate(&self) -> bool {
        self.speculate
    }
    fn empty(&self, _: &Values) -> ValueId {
        self.zero().unwrap_or_else(|| {
            miss(format_args!("empty of accumulator {}", self.field));
            ValueId::NIL
        })
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        if !self.exact {
            let s = v.seq_concat(self.marked(v, a), self.marked(v, b));
            return match v.seq_len(s) {
                0 => self.zero().unwrap_or(ValueId::NIL),
                1 => formal_leaves(v, s).map_or(s, |(_, l)| l[0]),
                _ => {
                    self.book.formal.lock().unwrap().entry(s.raw()).or_insert(None);
                    s
                }
            };
        }
        if let Some(r) = self
            .book
            .monoid
            .get(&key(&[&self.field, &json!("combine"), &json!([a, b])]))
        {
            return de(r);
        }
        match (&*v.get(a), &*v.get(b)) {
            (Value::Int(x), Value::Int(y)) => v.int(x + y),
            (Value::List(x), Value::List(y)) => v.list(x.iter().chain(y.iter()).copied().collect()),
            _ => v.seq_concat(a, b),
        }
    }
}

impl Book {
    /// Whether answer `got` (formal folds in it) is recorded answer `rec`:
    /// a formal fold is the value it was matched with before, else the
    /// fold of its contributions by the recorded combines, else anything.
    fn same(&self, v: &Values, got: &J, rec: &J) -> bool {
        if got == rec {
            return true;
        }
        match (got, rec) {
            (J::Number(g), J::Number(r)) => {
                let Some(g) = g.as_u64() else { return false };
                match self.formal_value(v, g) {
                    Some(Some(x)) => r.as_u64() == Some(x),
                    Some(None) => true,
                    None => false,
                }
            }
            (J::Array(g), J::Array(r)) => g.len() == r.len() && g.iter().zip(r).all(|(g, r)| self.same(v, g, r)),
            (J::Object(g), J::Object(r)) => {
                g.len() == r.len() && g.iter().all(|(k, g)| r.get(k).is_some_and(|r| self.same(v, g, r)))
            }
            _ => false,
        }
    }
    /// For formal fold `g`: what it is, if known (`None`: not formal).
    fn formal_value(&self, v: &Values, g: u64) -> Option<Option<u64>> {
        if let Some(x) = *self.formal.lock().unwrap().get(&g)? {
            return Some(Some(x));
        }
        let (field, leaves) = formal_leaves(v, de(&json!(g)))?;
        let mut acc = *leaves.first()?;
        for &x in &leaves[1..] {
            match self.monoid.get(&key(&[&field, &json!("combine"), &json!([acc, x])])) {
                Some(r) => acc = de(r),
                None => return Some(None),
            }
        }
        self.formal.lock().unwrap().insert(g, Some(acc.raw()));
        Some(Some(acc.raw()))
    }
    /// Notes what the formal folds in `got` are, as `rec` has them.
    fn learn(&self, v: &Values, got: &J, rec: &J) {
        match (got, rec) {
            (J::Number(g), J::Number(r)) => {
                if let (Some(g), Some(r)) = (g.as_u64(), r.as_u64())
                    && g != r
                    && self.formal_value(v, g) == Some(None)
                {
                    self.formal.lock().unwrap().insert(g, Some(r));
                }
            }
            (J::Array(g), J::Array(r)) => g.iter().zip(r).for_each(|(g, r)| self.learn(v, g, r)),
            (J::Object(g), J::Object(r)) => g.iter().for_each(|(k, g)| {
                if let Some(r) = r.get(k) {
                    self.learn(v, g, r);
                }
            }),
            _ => {}
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
                match b.proj.get(&key(&[&json!(id), &json!(v)])) {
                    Some(r) => de(r),
                    None => {
                        miss(format_args!("projection {id} of {v:?}"));
                        v
                    }
                }
            }));
        }
        "declare_accumulator" => {
            eng.declare_accumulator(
                de(a),
                Arc::new(PlayMonoid {
                    book: book.clone(),
                    field: a.clone(),
                    exact: book.exact.contains(&a.to_string()),
                    speculate: want.as_bool().unwrap_or(false),
                    zero: book.monoid.get(&key(&[a, &json!("empty"), &J::Null])).map(de),
                }),
            );
        }
        "declare_accumulator_prefix" => {
            // (The engine resolves a prefix per field; each recorded field
            // under it is declared on its own, answering from its lines.)
            let prefix = a[0].as_str().unwrap_or("");
            for name in book.strs.iter().filter(|n| n.starts_with(prefix)) {
                let f = Field(eng.values().str(name));
                let fj = json!(f);
                eng.declare_accumulator(
                    f,
                    Arc::new(PlayMonoid {
                        book: book.clone(),
                        field: fj.clone(),
                        exact: book.exact.contains(&fj.to_string()),
                        speculate: want.as_bool().unwrap_or(false),
                        zero: book.monoid.get(&key(&[&fj, &json!("empty"), &J::Null])).map(de),
                    }),
                );
            }
        }
        "set_env_provider" => {
            let b = book.clone();
            eng.set_env_provider(Box::new(move |_: &Values, f: Field| {
                b.env.get(&json!(f).to_string()).map(now).and_then(|l| {
                    // A recording with `known` says which null it was; an
                    // older one wrote "absent" and "unknown" alike, and its
                    // build completed: absent.
                    match l["known"].as_bool() {
                        Some(false) => None,
                        _ => Some(de::<Option<ValueId>>(&l["r"])),
                    }
                })
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
        "declare_loopbacks" => eng.declare_loopbacks(&de::<Vec<(Field, ValueId)>>(&g(0))),
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
        "set_stable" => eng.set_stable(de(&g(0))),
        "set_heights" => eng.set_heights(de(&g(0))),
        "set_round_cap_policy" => eng.set_round_cap_policy(de::<CapPolicy>(&g(0))),
        "refresh_where" => {
            let texts: std::collections::HashSet<(OpId, Vec<ValueId>)> = de(a);
            eng.refresh_where(&|op, args, _| texts.contains(&(op, args.to_vec())));
        }
        "pin" | "unpin" | "evaluate_begin" | "evaluate_end" => {}
        other => eprintln!("play: skipped engine call {other}"),
    }
}

/// The accumulators whose recorded combines are all sums, list or
/// sequence concatenations (judged from the recorded values).
fn exact_fields(lines: &[J], combines: &[&J]) -> std::collections::HashSet<String> {
    let vals: HashMap<u64, &J> = lines
        .iter()
        .filter(|l| l["k"] == "val")
        .filter_map(|l| Some((l["id"].as_u64()?, l)))
        .collect();
    // A value's elements, if it is a sequence (`nil`: none).
    let elems = |x: &J| -> Option<Vec<J>> {
        let l = vals.get(&x.as_u64()?)?;
        match l["t"].as_str()? {
            "nil" => Some(Vec::new()),
            "seq" => l["x"].as_array().cloned(),
            _ => None,
        }
    };
    let is = |a: &J, b: &J, r: &J| -> Option<bool> {
        let (va, vb, vr) = (vals.get(&a.as_u64()?)?, vals.get(&b.as_u64()?)?, vals.get(&r.as_u64()?)?);
        Some(match (va["t"].as_str()?, vb["t"].as_str()?) {
            ("int", "int") => vr["t"] == "int" && vr["x"].as_i64() == Some(va["x"].as_i64()? + vb["x"].as_i64()?),
            ("list", "list") => {
                let mut x = va["x"].as_array()?.clone();
                x.extend(vb["x"].as_array()?.iter().cloned());
                vr["t"] == "list" && vr["x"].as_array() == Some(&x)
            }
            _ => {
                let mut x = elems(a)?;
                x.extend(elems(b)?);
                elems(r)? == x
            }
        })
    };
    let mut ok: HashMap<String, bool> = HashMap::new();
    for l in combines {
        let yes = is(&l["a"][0], &l["a"][1], &l["r"]).unwrap_or(false);
        *ok.entry(l["f"].to_string()).or_insert(true) &= yes;
    }
    ok.into_iter().filter(|e| e.1).map(|e| e.0).collect()
}

/// A run's calls without the calls a public `Ctx` call made inside and a
/// recording before format 2 wrote too: each right before the call that
/// made it.
fn single_calls(calls: Vec<J>) -> Vec<J> {
    let mut out: Vec<J> = Vec::with_capacity(calls.len());
    for c in calls {
        let (m, a, r) = (c[0].as_str().unwrap_or(""), &c[1], &c[2]);
        loop {
            let Some(p) = out.last() else { break };
            let (pm, pa, pr) = (p[0].as_str().unwrap_or(""), &p[1], &p[2]);
            let inner = match (pm, m) {
                // `define`, `push_contribution`: through `define_with`;
                // `take_prev_contribution` defines its field the same way.
                ("define_with", "define" | "push_contribution") => pa[0] == a[0] && pa[1] == a[1],
                ("define_with", "take_prev_contribution") => pa[0] == a[0],
                // `follow_after` and `follow_after_any` placing it as `follow`.
                ("follow", "follow_after" | "follow_after_any") => pa[0] == a[1] && pa[1] == a[2] && pr == r,
                // `acc_range` is `acc_range_to` the node.
                ("acc_range_to", "acc_range") => pa[0] == a[0] && pa[1] == a[1] && pa[2].is_null() && pr == r,
                // `claimed`, for a claim the run made: `following` 0, 1, ..
                ("following", "claimed") => {
                    let n = out.iter().rev().take_while(|p| p[0] == "following").count();
                    let tail = &out[out.len() - n..];
                    let ks: Vec<Option<u64>> = tail.iter().map(|p| p[1][0].as_u64()).collect();
                    let got: Vec<&J> = tail.iter().map(|p| &p[2]).filter(|x| !x.is_null()).collect();
                    let want: Vec<&J> = r.as_array().map(|v| v.iter().collect()).unwrap_or_default();
                    if ks.iter().enumerate().all(|(i, k)| *k == Some(i as u64)) && got == want {
                        out.truncate(out.len() - n);
                    }
                    false
                }
                _ => false,
            };
            if !inner {
                break;
            }
            out.pop();
        }
        out.push(c);
    }
    out
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

/// The command line.
struct Opts {
    argv: Vec<String>,
    threads: usize,
    budget: usize,
    spec: Speculation,
}

impl Opts {
    fn opt(&self, name: &str) -> Option<String> {
        self.argv
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.argv.get(i + 1))
            .cloned()
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let path = argv
        .get(1)
        .expect("usage: play REC [--engine E] [--threads T] [--spec off|predicted|all] [--budget B]")
        .clone();
    let mut o = Opts {
        argv,
        threads: 1,
        budget: 64,
        spec: Speculation::Off,
    };
    o.threads = o.opt("--threads").map_or(1, |s| s.parse().unwrap());
    o.budget = o.opt("--budget").map_or(64, |s| s.parse().unwrap());
    o.spec = match o.opt("--spec").as_deref() {
        Some("all") => Speculation::All,
        Some("predicted") => Speculation::Predicted,
        _ => Speculation::Off,
    };
    let text = std::fs::read_to_string(path).expect("read recording");
    let lines: Vec<J> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    // The engines, in the order they first evaluated: the one asked for,
    // else every one.
    let mut engines: Vec<(u64, u64, u64)> = Vec::new();
    for l in &lines {
        let Some(e) = l["e"].as_u64() else { continue };
        let at = match engines.iter().position(|x| x.0 == e) {
            Some(k) => k,
            None if l["k"] == "api" && l["f"] == "evaluate_begin" => {
                engines.push((e, 0, 0));
                engines.len() - 1
            }
            None => continue,
        };
        if l["k"] == "api" && l["f"] == "evaluate_begin" {
            engines[at].1 += 1;
        } else if l["k"] == "run" {
            engines[at].2 += 1;
        }
    }
    if engines.is_empty() {
        eprintln!("play: the recording has no evaluation");
        std::process::exit(2);
    }
    for (e, n, r) in &engines {
        println!("engine {e}: {n} evaluations, {r} runs recorded");
    }
    if let Some(e) = o.opt("--engine").map(|s| s.parse::<u64>().unwrap()) {
        engines.retain(|x| x.0 == e);
        if engines.is_empty() {
            eprintln!("play: no evaluation of engine {e} in the recording");
            std::process::exit(2);
        }
    }
    // A recording before format 2 wrote a `Ctx` call also as the public
    // calls it made inside.
    let v2 = lines.iter().any(|l| l["k"] == "rec" && l["v"].as_u64() >= Some(2));
    let mut evals = 0;
    for &(e, _, _) in &engines {
        let (d0, m0) = (DIVERGED.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed));
        println!("replaying engine {e}");
        let n = replay(&lines, e, v2, &o);
        println!(
            "engine {e}: replayed {n} evaluations; {} divergent runs, {} unrecorded hook answers",
            DIVERGED.load(Ordering::Relaxed) - d0,
            MISSES.load(Ordering::Relaxed) - m0
        );
        evals += n;
    }
    println!(
        "replayed {evals} evaluations; {} divergent runs, {} unrecorded hook answers",
        DIVERGED.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed)
    );
    if defuse::obs::enabled() {
        print!("probes:\n{}", defuse::obs::snapshot());
    }
}


/// Replays engine `engine` of the recording in a fresh engine; returns the
/// number of evaluations.
#[allow(clippy::too_many_lines)]
fn replay(all: &[J], engine: u64, v2: bool, o: &Opts) -> usize {
    let (threads, budget, spec) = (o.threads, o.budget, o.spec);
    let argv = &o.argv;
    let opt = |name: &str| o.opt(name);
    // Values are recorded once, under the engine that first made them: a
    // client may share its value table between engines.
    let lines: Vec<J> = all
        .iter()
        .filter(|l| l["e"].as_u64() == Some(engine) || l["k"] == "val")
        .cloned()
        .collect();
    let mut book = Book::default();
    // Each node's last stopped run: from its start (`None`: not known),
    // and its line.
    let mut stopped: HashMap<String, (Option<Vec<J>>, usize)> = HashMap::new();
    // Identical runs of a text are one candidate: (text, run) -> its index.
    let mut merged: HashMap<String, usize> = HashMap::new();
    let mut add = |book: &mut Book, text: &String, calls: Vec<J>, r: &J, node: &J, origin: usize, resumes: Option<usize>| {
        let runs = book.runs.entry(text.clone()).or_default();
        let k = *merged
            .entry(format!("{text}|{}", json!([calls, r])))
            .or_insert_with(|| {
                runs.push(Run {
                    calls,
                    r: r.clone(),
                    ..Run::default()
                });
                runs.len() - 1
            });
        runs[k].origins.push(origin);
        runs[k].resumes.extend(resumes);
        if !runs[k].nodes.contains(node) {
            runs[k].nodes.push(node.clone());
        }
    };
    let mut combines: Vec<&J> = Vec::new();
    for (at, l) in lines.iter().enumerate() {
        match l["k"].as_str().unwrap_or("") {
            "st" => {
                let m = &l["m"];
                if matches!(m.as_str(), Some("claim" | "claim_limit" | "until_predict")) {
                    book.takes
                        .entry(key(&[&l["op"], m, &l["a"]]))
                        .or_default()
                        .push((at, de(&l["x"]), l["r"].clone()));
                } else {
                    book.statics
                        .entry(key(&[&l["op"], m, &l["a"], &l["x"]]))
                        .or_default()
                        .push((at, l["r"].clone()));
                }
            }
            "run" => {
                let mut calls = l["calls"].as_array().cloned().unwrap_or_default();
                if !v2 {
                    calls = single_calls(calls);
                }
                let (r, text) = (&l["r"], key(&[&l["op"], &l["a"]]));
                // A resumed run (its first call answered `parked` true) is
                // recorded from where it stopped, after its node's stopped
                // run; when the replay restarts it instead, the run from
                // its start is the stopped run's up to its blocked read
                // (answered now: any answer) and the resumed calls.
                let resumed = calls.first().is_some_and(|c| c[0] == "parked" && c[2] == true);
                let node = l["node"].to_string();
                let prev = if resumed { stopped.remove(&node) } else { None };
                let from_start = match &prev {
                    None if resumed => None,
                    None => Some(calls.clone()),
                    Some((p, _)) => p.as_ref().filter(|p| p.len() >= 2 && (p[p.len() - 1][0] == "park" || p[p.len() - 1][0] == "park_continuing")).map(|p| {
                        let mut c: Vec<J> = p[..p.len() - 1].to_vec();
                        if let Some(last) = c.last_mut() {
                            last[2] = json!("*");
                        }
                        c.extend(calls[1..].iter().cloned());
                        c
                    }),
                };
                if let (true, Some(c)) = (resumed, &from_start) {
                    add(&mut book, &text, c.clone(), r, &l["node"], at, None);
                }
                if *r == json!("stop") {
                    stopped.insert(node, (from_start, at));
                } else {
                    stopped.remove(&node);
                }
                add(&mut book, &text, calls, r, &l["node"], at, prev.map(|p| p.1));
            }
            "mono" => {
                let a = if l["m"] == "empty" {
                    J::Null
                } else {
                    combines.push(l);
                    l["a"].clone()
                };
                book.monoid.insert(key(&[&l["f"], &l["m"], &a]), l["r"].clone());
            }
            "proj" => {
                book.proj.insert(key(&[&l["p"], &l["a"]]), l["r"].clone());
            }
            "val" if l["t"] == "str" => {
                book.strs.extend(l["x"].as_str().map(str::to_string));
            }
            "env" => {
                book.env.entry(l["f"].to_string()).or_default().push((at, l.clone()));
            }
            _ => {}
        }
    }
    book.exact = exact_fields(&lines, &combines);
    let book = Arc::new(book);
    let units = Arc::new(Mutex::new(Vec::new()));
    let mut eng = Engine::new();
    if argv.iter().any(|a| a == "--no-heights") {
        eng.set_heights(false);
    }
    // `--stable N`: the N fields the recorded runs read most are marked
    // stable (`Engine::set_stable`).
    if let Some(n) = opt("--stable").map(|s| s.parse::<usize>().unwrap()) {
        let mut reads: std::collections::HashMap<String, (J, u64)> = std::collections::HashMap::new();
        for runs in book.runs.values() {
            for run in runs {
                for c in &run.calls {
                    if c[0] == "use_field" {
                        let f = c[1][0].clone();
                        reads.entry(f.to_string()).or_insert((f, 0)).1 += 1;
                    }
                }
            }
        }
        let mut top: Vec<(J, u64)> = reads.into_values().collect();
        top.sort_by_key(|e| std::cmp::Reverse(e.1));
        for (f, k) in top.into_iter().take(n) {
            eprintln!("play: stable {f} ({k} recorded reads)");
            eng.set_stable(de(&f));
        }
    }
    let mut evals = 0;
    // (Pushed nodes and op names, for `--edits`.)
    let mut pushes: Vec<(J, J, J)> = Vec::new();
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
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
                // Gather this evaluation's absorbed batches; it answers as
                // the recording did by its end.
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
                        AT.store(i - 1, Ordering::Relaxed);
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
                if let Ok(rep) = &r
                    && std::env::var_os("PLAY_DAG").is_some()
                {
                    sched_study(&eng, rep);
                }
                if let Ok(rep) = &r {
                    println!(
                        "  runs {} stops {} rounds {} steps {}",
                        rep.nodes_run, rep.stops, rep.rounds, rep.steps
                    );
                    println!("  restarted_by {:?} resumed {}", rep.restarted_by, rep.resumed);
                    // Where the time went, and two bounds on the wall time:
                    // the busy time spread over the workers, and the
                    // critical path at the mean run's cost.
                    let ms = |ns: u64| ns as f64 / 1e6;
                    let (mut op, mut commit, mut check, mut idle) = (0, 0, 0, 0);
                    for t in &rep.round_times {
                        (op, commit, check, idle) =
                            (op + t.op_ns, commit + t.commit_ns, check + t.check_ns, idle + t.idle_ns);
                    }
                    if rep.round_times.len() > 1 {
                        for (k, t) in rep.round_times.iter().enumerate() {
                            let b = t.op_ns + t.commit_ns + t.check_ns;
                            println!(
                                "  round {}: wall {:.1} ms; op {:.1} commit {:.1} check {:.1} idle {:.1}; busy/{threads} {:.1}",
                                k + 1,
                                ms(t.wall_ns),
                                ms(t.op_ns),
                                ms(t.commit_ns),
                                ms(t.check_ns),
                                ms(t.idle_ns),
                                ms(b) / threads as f64
                            );
                        }
                    }
                    let busy = op + commit + check;
                    let per_run = op as f64 / rep.nodes_run.max(1) as f64;
                    println!(
                        "  op {:.1} ms commit {:.1} check {:.1} idle {:.1}; op per run {:.1} us",
                        ms(op),
                        ms(commit),
                        ms(check),
                        ms(idle),
                        per_run / 1e3
                    );
                    println!(
                        "  realised critical path: {} runs, {:.1} ms",
                        rep.realised_critical_path,
                        rep.realised_critical_time.as_secs_f64() * 1e3
                    );
                    if std::env::var_os("DEFUSE_RCHAIN").is_some() {
                        // The realised chain, deepest first: why each run
                        // came after the next.
                        for (n, d, k) in eng.realised_chain() {
                            let what = eng.node_info(n).map_or(String::new(), |i| {
                                let a: Vec<String> = i.args.iter().take(3).map(|&v| eng.values().render(v)).collect();
                                format!("{} {}", i.op, a.join(" ")).chars().take(90).collect()
                            });
                            println!("    chain {d:>4} {k:<24} {what}");
                        }
                    }
                    println!(
                        "  bounds: busy/{threads} {:.1} ms, critical path {} runs = {:.1} ms",
                        ms(busy) / threads as f64,
                        rep.critical_path,
                        f64::from(rep.critical_path) * per_run / 1e6
                    );
                }
            }
            "api" => {
                AT.store(i - 1, Ordering::Relaxed);
                if l["f"] == "push" {
                    pushes.push((l["r"].clone(), l["a"][1].clone(), l["a"][2].clone()));
                } else if l["f"] == "register" {
                    names.insert(l["r"].to_string(), l["a"][0].as_str().unwrap_or("").to_string());
                }
                apply(&mut eng, l, &book, &units);
            }
            _ => {}
        }
    }
    // `--edits N [--edit-op NAME]`: after the replay, N edits of a node of
    // that op (default exec) spread over the document, each its own text
    // again (re-run on the recorded answers; an unchanged output stops the
    // change there), timed one evaluation each.
    if let Some(n) = opt("--edits").map(|s| s.parse::<usize>().unwrap()) {
        let opname = opt("--edit-op").unwrap_or_else(|| "exec".to_string());
        let of_op: Vec<&(J, J, J)> = pushes
            .iter()
            .filter(|p| names.get(&p.1.to_string()).is_some_and(|x| *x == opname))
            .collect();
        let mut times = Vec::new();
        for i in 0..n.min(of_op.len()) {
            let (node, op, args) = of_op[of_op.len() * (i + 1) / (n + 1)];
            eng.replace(de(node), de(op), de(args));
            let mut sched = Greedy::speculative(spec, budget);
            let t0 = std::time::Instant::now();
            let r = eng.evaluate(&mut sched, &mut Threads(threads));
            let dt = t0.elapsed();
            times.push(dt.as_secs_f64() * 1e3);
            if let Ok(rep) = &r {
                let sum = |g: fn(&defuse::RoundTimes) -> u64| rep.round_times.iter().map(g).sum::<u64>() as f64 / 1e6;
                println!(
                    "edit {}: {:.2} ms, runs {} stops {}; op {:.2} commit {:.2} check {:.2} ms",
                    i + 1,
                    dt.as_secs_f64() * 1e3,
                    rep.nodes_run,
                    rep.stops,
                    sum(|t| t.op_ns),
                    sum(|t| t.commit_ns),
                    sum(|t| t.check_ns),
                );
            } else {
                println!("edit {}: {r:?}", i + 1);
            }
        }
        times.sort_by(f64::total_cmp);
        if !times.is_empty() {
            println!(
                "edits: median {:.2} ms over {} ({} {opname} nodes)",
                times[times.len() / 2],
                times.len(),
                of_op.len()
            );
        }
    }
    evals
}

fn short(j: &J) -> String {
    let s = j.to_string();
    if s.len() > 300 { format!("{}…", &s[..300]) } else { s }
}

/// The best any scheduler could do on this evaluation's dependency graph
/// with these run times (scaled to PLAY_OP_TOTAL_MS in all when set: a
/// live run's op time): the critical path, and a list schedule (longest
/// path to the end first) on P workers, with commits free, each taking
/// PLAY_COMMIT_US on one committer (a serial merge), or each taking it on
/// the worker that ran the node (commits in parallel: a mergeless engine).
fn sched_study(eng: &defuse::Engine, rep: &defuse::Report) {
    use std::collections::{BinaryHeap, HashMap, VecDeque};
    let g = eng.dependency_graph();
    let idx: HashMap<defuse::NodeId, usize> = g.iter().enumerate().map(|(i, (n, _))| (*n, i)).collect();
    let times: HashMap<defuse::NodeId, f64> = rep.run_times.iter().map(|(n, d)| (*n, d.as_secs_f64() * 1e3)).collect();
    let n = g.len();
    let mut t: Vec<f64> = g.iter().map(|(x, _)| times.get(x).copied().unwrap_or(0.0)).collect();
    if let Some(want) = std::env::var("PLAY_OP_TOTAL_MS").ok().and_then(|s| s.parse::<f64>().ok()) {
        let have: f64 = t.iter().sum();
        if have > 0.0 {
            t.iter_mut().for_each(|x| *x *= want / have);
        }
    }
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    let mut edges = 0;
    for (i, (_, ds)) in g.iter().enumerate() {
        for d in ds {
            if let Some(&j) = idx.get(d) {
                succ[j].push(i);
                indeg[i] += 1;
                edges += 1;
            }
        }
    }
    // A topological order; nodes left in cycles lose their remaining
    // in-edges (counted).
    let mut deg = indeg.clone();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut q: VecDeque<usize> = (0..n).filter(|&i| deg[i] == 0).collect();
    let mut broken = 0;
    let mut seen = vec![false; n];
    loop {
        while let Some(i) = q.pop_front() {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            order.push(i);
            for &s in &succ[i] {
                deg[s] = deg[s].saturating_sub(1);
                if deg[s] == 0 {
                    q.push_back(s);
                }
            }
        }
        match (0..n).find(|&i| !seen[i]) {
            Some(i) => {
                broken += 1;
                deg[i] = 0;
                q.push_back(i);
            }
            None => break,
        }
    }
    let mut pos = vec![0usize; n];
    for (k, &i) in order.iter().enumerate() {
        pos[i] = k;
    }
    // Longest path from each node to the end (its own time included),
    // along edges that go forward in the order.
    let mut bl = vec![0.0f64; n];
    for &i in order.iter().rev() {
        let m = succ[i].iter().filter(|&&s| pos[s] > pos[i]).map(|&s| bl[s]).fold(0.0, f64::max);
        bl[i] = t[i] + m;
    }
    let crit = bl.iter().copied().fold(0.0, f64::max);
    let total: f64 = t.iter().sum();
    // The critical chain's length in runs.
    let mut len = 0;
    let mut cur = (0..n).max_by(|&a, &b| bl[a].total_cmp(&bl[b]));
    while let Some(i) = cur {
        len += 1;
        cur = succ[i].iter().copied().filter(|&s| pos[s] > pos[i]).max_by(|&a, &b| bl[a].total_cmp(&bl[b]));
    }
    println!("  dag: nodes {n} edges {edges} cycles broken {broken}; run time {total:.1} ms; critical path {crit:.1} ms over {len} runs");
    let commit_us: f64 = std::env::var("PLAY_COMMIT_US").ok().and_then(|s| s.parse().ok()).unwrap_or(30.0);
    // `c`: one committer's time per result (ms); `own`: each node's own
    // commit time, paid by its worker.
    let sim = |p: usize, c: f64, own: f64| -> f64 {
        // Ready by priority (longest path to the end first).
        #[derive(PartialEq)]
        struct R(f64, usize);
        impl Eq for R {}
        impl PartialOrd for R {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for R {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                self.0.total_cmp(&o.0).then(o.1.cmp(&self.1))
            }
        }
        let ns = |x: f64| (x * 1e6) as u64;
        let mut deg: Vec<usize> = (0..n).map(|i| g[i].1.iter().filter(|d| idx.get(d).is_some_and(|&j| pos[j] < pos[i])).count()).collect();
        let mut ready: BinaryHeap<R> = BinaryHeap::new();
        // Events (time, kind, node): kind 0 a node becomes ready, 1 a run
        // ends.
        let mut ev: BinaryHeap<std::cmp::Reverse<(u64, u8, usize)>> = (0..n).filter(|&i| deg[i] == 0).map(|i| std::cmp::Reverse((0, 0, i))).collect();
        let (mut now, mut free, mut committer, mut done) = (0u64, p, 0u64, 0usize);
        while done < n {
            let Some(std::cmp::Reverse((tm, kind, i))) = ev.pop() else { break };
            now = tm;
            if kind == 0 {
                ready.push(R(bl[i], i));
            } else {
                free += 1;
                done += 1;
                let end = if c > 0.0 {
                    committer = committer.max(now) + ns(c);
                    committer
                } else {
                    now
                };
                for &s in &succ[i] {
                    if pos[s] > pos[i] {
                        deg[s] -= 1;
                        if deg[s] == 0 {
                            ev.push(std::cmp::Reverse((end, 0, s)));
                        }
                    }
                }
            }
            // Start what can start now (after every event of this time).
            if ev.peek().is_some_and(|e| e.0.0 == now) {
                continue;
            }
            while free > 0 {
                let Some(R(_, x)) = ready.pop() else { break };
                free -= 1;
                ev.push(std::cmp::Reverse((now + ns(t[x] + own), 1, x)));
            }
        }
        now.max(committer) as f64 / 1e6
    };
    let mut line = String::from("  ideal:");
    for p in [1usize, 2, 4, 8, 16, 32, 48] {
        line += &format!(" j{p} {:.1}/{:.1}/{:.1}", sim(p, 0.0, 0.0), sim(p, commit_us / 1e3, 0.0), sim(p, 0.0, commit_us / 1e3));
    }
    println!("{line}  (ms; free commits / one committer at {commit_us} us each / each worker its own at {commit_us} us)");
    // The chain with each node's own commit: the mergeless floor.
    let mut bl2 = vec![0.0f64; n];
    for &i in order.iter().rev() {
        let m = succ[i].iter().filter(|&&s| pos[s] > pos[i]).map(|&s| bl2[s]).fold(0.0, f64::max);
        bl2[i] = t[i] + commit_us / 1e3 + m;
    }
    println!(
        "  chain: {len} of {n} nodes ({:.2}%); critical path {crit:.1} ms, {:.1} ms with own commits; run time {total:.1} ms",
        100.0 * len as f64 / n.max(1) as f64,
        bl2.iter().copied().fold(0.0, f64::max)
    );
}
