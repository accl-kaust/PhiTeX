//! Toy D: per-key loop fields seeded from a previous run, a planner
//! folding a range into piece nodes whose count fields are read back as
//! loop fields, and environment-provider reads; every count exact.
//!
//! Program (one unit), in order:
//! - 200 items `["item", i, a, w]`. Item i reads 1–2 environment fields
//!   `file:i:e` (the provider answers them in place), reads 0–3 per-key
//!   loop fields `lp:k` whose publisher comes LATER, and a few items read
//!   a piece count `pc:p` (a loop field too). It publishes 0–3 loop fields
//!   `lp:k` (value: a hash of `a`, the key, everything it read), contributes
//!   `[i, w]` to the sequence accumulator `w` (declared statically), and
//!   outputs a hash of `a` and everything it read.
//! - Three of the items instead read an environment field `decl:d` the
//!   provider does not know; the client then declares the loop field
//!   `lbx:d` and supplies `decl:d`, and the item reads `lbx:d` (they
//!   publish nothing and nobody reads them).
//! - A planner `["plan"]` at the end folds `w` over the whole unit and
//!   unfolds 40 pieces `["piece", p, count]`, `count` = the sum of `w` of
//!   items `5p..5p+5`. Piece p publishes `pc:p` = `count`.
//!
//! The loop fields are declared with the fixpoint of a PREVIOUS program
//! (six items' `a` and three items' `w` differ), as a client seeds them
//! from its last session. The client drives the engine as ours does:
//! a fresh provider before every evaluation, and after an evaluation that
//! asked for anything (environment waits) it declares the asked loop
//! fields, then supplies the FULL environment map (the provider's answers
//! mirrored, plus what was asked) and the full absent set, and evaluates
//! again. On an edit it replaces the node, re-declares the three `lbx:d`
//! with their same initial value (as for re-made nodes) and supplies the
//! same full environment map again. Loop start: Previous.
//!
//! The oracle simulates an ideal engine round by round: a node runs in a
//! round iff it was edited (round 1), its fold changed (the planner), its
//! arguments changed (a piece), or a value it reads differs from what its
//! last run read. Asserts (equality, never a bound):
//! - cold: per node, completed runs equal the oracle's; the first
//!   evaluation's loop rounds equal the fixpoint depth of the seeds
//!   (rounds in which a node runs); the second evaluation runs exactly the
//!   three waiting items, once each, in 1 round;
//! - each one-value edit: per node, completed runs equal the oracle's (the
//!   edited item, plus the nodes whose read values changed, once per round
//!   in which they changed); rerun causes: no Environment at all, Loop
//!   exactly for the items whose loop-field reads changed, Range exactly
//!   for the planner when its fold changed;
//! - every output equals the oracle's fixpoint, after every evaluation.
//!
//! An evaluation error (a node over `Limits::runs_per_node`, say) is
//! reported with its node and causes. Run with `--nocapture` for the
//! numbers.

#![allow(clippy::all, clippy::pedantic)] // (Phi's toy, kept as written)
#![allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, LoopStart, NodeId, OpId, Operation, Report, Stop, Value, ValueId, Values};

const N: usize = 200;
const PIECES: usize = 40;
const PER_PIECE: usize = N / PIECES;
const DECL: [usize; 3] = [50, 120, 190];
/// Logical node ids: items 0..N, the planner N, piece p N + 1 + p.
const PLAN: usize = N;
const NODES: usize = N + 1 + PIECES;

// ------------------------------------------------------------ the program

#[derive(Clone)]
struct Item {
    defs: Vec<usize>,
    reads: Vec<usize>,
    pcs: Vec<usize>,
    envs: Vec<usize>,
    decl: Option<usize>,
}

struct Prog {
    items: Vec<Item>,
    keys: usize,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn prog() -> Prog {
    let mut g = Rng(0x5eed);
    let mut items: Vec<Item> = Vec::new();
    let mut definer: Vec<usize> = Vec::new();
    for i in 0..N {
        let decl = DECL.iter().position(|&d| d == i);
        let nd = if decl.is_some() { 0 } else { g.below(4) };
        let defs: Vec<usize> = (0..nd)
            .map(|_| {
                definer.push(i);
                definer.len() - 1
            })
            .collect();
        let envs = if decl.is_some() {
            Vec::new()
        } else {
            (0..1 + g.below(2)).collect()
        };
        items.push(Item {
            defs,
            reads: Vec::new(),
            pcs: Vec::new(),
            envs,
            decl,
        });
    }
    for i in 0..N {
        if items[i].decl.is_some() {
            continue;
        }
        let later: Vec<usize> = (0..definer.len()).filter(|&k| definer[k] > i).collect();
        if !later.is_empty() {
            let nr = g.below(4);
            let mut r = BTreeSet::new();
            for _ in 0..nr {
                // mostly near: chains of a few hops, as real readers have
                let k = later[g.below(later.len().min(12))];
                r.insert(k);
            }
            items[i].reads = r.into_iter().collect();
        }
        if i % 20 == 3 {
            items[i].pcs = vec![g.below(PIECES)];
        }
    }
    Prog {
        items,
        keys: definer.len(),
    }
}

fn fnv(xs: &[i64]) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in xs {
        for b in x.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    (h >> 2) as i64
}

fn env_val(i: usize, e: usize) -> i64 {
    fnv(&[7, i as i64, e as i64])
}

fn lbx_init(d: usize) -> i64 {
    900 + d as i64
}

// ------------------------------------------------------------ the oracle

/// The loop fields: `lp:k` at k, `pc:p` at keys + p, `lbx:d` at keys + PIECES + d.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Loops(Vec<i64>);

impl Prog {
    fn lp(&self, k: usize) -> usize {
        k
    }
    fn pc(&self, p: usize) -> usize {
        self.keys + p
    }
    fn lbx(&self, d: usize) -> usize {
        self.keys + PIECES + d
    }
    fn nloops(&self) -> usize {
        self.keys + PIECES + DECL.len()
    }

    /// What item i reads at loop values `l`: (loop-field reads, environment reads).
    fn read(&self, i: usize, l: &Loops) -> (Vec<i64>, Vec<i64>) {
        let it = &self.items[i];
        let mut r: Vec<i64> = it.reads.iter().map(|&k| l.0[self.lp(k)]).collect();
        r.extend(it.pcs.iter().map(|&p| l.0[self.pc(p)]));
        if let Some(d) = it.decl {
            r.push(l.0[self.lbx(d)]);
        }
        let e: Vec<i64> = it.envs.iter().map(|&e| env_val(i, e)).collect();
        (r, e)
    }

    fn publish(&self, i: usize, a: i64, r: &[i64], e: &[i64]) -> Vec<(usize, i64)> {
        self.items[i]
            .defs
            .iter()
            .map(|&k| {
                let mut xs = vec![a, k as i64];
                xs.extend_from_slice(r);
                xs.extend_from_slice(e);
                (self.lp(k), fnv(&xs))
            })
            .collect()
    }

    fn output(a: i64, r: &[i64], e: &[i64]) -> i64 {
        let mut xs = vec![a, -1];
        xs.extend_from_slice(r);
        xs.extend_from_slice(e);
        fnv(&xs)
    }

    fn counts(ws: &[i64]) -> Vec<i64> {
        (0..PIECES)
            .map(|p| ws[p * PER_PIECE..(p + 1) * PER_PIECE].iter().sum())
            .collect()
    }
}

/// The oracle's view of the program's state after an evaluation.
#[derive(Clone)]
struct State {
    loops: Loops,
    /// What each item's last run read (`None`: never ran).
    last: Vec<Option<Vec<i64>>>,
    counts: Option<Vec<i64>>,
    outs: Vec<Option<i64>>,
}

/// Simulates an ideal engine over one evaluation: per-node runs, rounds
/// in which a node ran, the items re-run for a loop-field read, whether
/// the planner re-ran for its fold. `edited`: the item re-texted;
/// `waiting`: items that cannot run (environment unknown).
struct Sim {
    runs: Vec<u64>,
    rounds: u64,
    loop_readers: BTreeSet<usize>,
    range: bool,
}

fn simulate(p: &Prog, st: &mut State, a: &[i64], w: &[i64], edited: Option<usize>, waiting: &BTreeSet<usize>) -> Sim {
    let mut runs = vec![0u64; NODES];
    let mut loop_readers = BTreeSet::new();
    let mut range = false;
    let mut rounds = 0;
    let mut first = true;
    loop {
        let mut next = st.loops.clone();
        let mut ran = false;
        for i in 0..N {
            if waiting.contains(&i) {
                continue;
            }
            let (r, e) = p.read(i, &st.loops);
            let changed = st.last[i].as_ref() != Some(&r);
            let text = first && (edited == Some(i) || st.last[i].is_none());
            if !(text || changed) {
                continue;
            }
            if changed && st.last[i].is_some() && !text {
                loop_readers.insert(i);
            }
            runs[i] += 1;
            ran = true;
            for (f, x) in p.publish(i, a[i], &r, &e) {
                next.0[f] = x;
            }
            st.outs[i] = Some(Prog::output(a[i], &r, &e));
            st.last[i] = Some(r);
        }
        if first {
            let c = Prog::counts(w);
            if st.counts.as_ref() != Some(&c) {
                if st.counts.is_some() {
                    range = true;
                }
                runs[PLAN] += 1;
                ran = true;
                for q in 0..PIECES {
                    if st.counts.as_ref().map(|o| o[q]) != Some(c[q]) {
                        runs[PLAN + 1 + q] += 1;
                        next.0[p.pc(q)] = c[q];
                    }
                }
                st.counts = Some(c);
            }
        }
        first = false;
        if ran {
            rounds += 1;
        }
        if next == st.loops {
            break;
        }
        st.loops = next;
    }
    Sim {
        runs,
        rounds,
        loop_readers,
        range,
    }
}

/// The fixpoint of the program (from any start: every chain goes forward).
fn fixpoint(p: &Prog, a: &[i64], w: &[i64]) -> Loops {
    let mut st = State {
        loops: Loops(vec![0; p.nloops()]),
        last: vec![None; N],
        counts: None,
        outs: vec![None; N],
    };
    for d in 0..DECL.len() {
        st.loops.0[p.lbx(d)] = lbx_init(d);
    }
    simulate(p, &mut st, a, w, None, &BTreeSet::new());
    st.loops
}

// ------------------------------------------------------------ the client

struct Toy {
    prog: Arc<Prog>,
    runs: Vec<AtomicU64>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    a.first().map_or_else(String::new, |&x| match &*v.get(x) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    })
}

fn int(v: &Values, x: ValueId) -> i64 {
    v.as_int(x).unwrap()
}

fn lp_field(v: &Values, k: usize) -> Field {
    v.field(&format!("lp:{k}"))
}
fn pc_field(v: &Values, p: usize) -> Field {
    v.field(&format!("pc:{p}"))
}
fn lbx_field(v: &Values, d: usize) -> Field {
    v.field(&format!("lbx:{d}"))
}

impl Toy {
    fn read_int(cx: &mut Ctx<'_>, f: Field) -> Result<i64, Stop> {
        let v = cx.values();
        Ok(cx.use_field(f)?.and_then(|x| v.as_int(x)).unwrap_or(i64::MIN))
    }
}

impl Operation for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        match tag(v, a).as_str() {
            "item" => Some(vec![(v.field("w"), v.seq_unit(v.list(vec![a[1], a[3]])))]),
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let p = &self.prog;
        match tag(v, &a).as_str() {
            "item" => {
                let i = usize::try_from(int(v, a[1])).unwrap();
                let it = &p.items[i];
                let mut e = Vec::new();
                for &k in &it.envs {
                    let x = cx.use_env(v.field(&format!("file:{i}:{k}")))?;
                    e.push(x.and_then(|x| v.as_int(x)).unwrap_or(i64::MIN));
                }
                let mut r = Vec::new();
                if let Some(d) = it.decl {
                    cx.use_env(v.field(&format!("decl:{d}")))?;
                }
                for &k in &it.reads {
                    r.push(Self::read_int(cx, lp_field(v, k))?);
                }
                for &q in &it.pcs {
                    r.push(Self::read_int(cx, pc_field(v, q))?);
                }
                if let Some(d) = it.decl {
                    r.push(Self::read_int(cx, lbx_field(v, d))?);
                }
                let ai = int(v, a[2]);
                for (f, x) in p.publish(i, ai, &r, &e) {
                    let k = f; // lp index = key
                    cx.publish(lp_field(v, k), v.int(x));
                }
                cx.contribute(v.field("w"), v.seq_unit(v.list(vec![a[1], a[3]])));
                self.runs[i].fetch_add(1, Relaxed);
                Ok(v.int(Prog::output(ai, &r, &e)))
            }
            "plan" => {
                let all = cx.acc_range(v.field("w"), None)?;
                let mut ws = vec![0i64; N];
                for e in v.seq_to_vec(all) {
                    if let Value::List(xs) = &*v.get(e) {
                        ws[usize::try_from(int(v, xs[0])).unwrap()] = int(v, xs[1]);
                    }
                }
                for (q, c) in Prog::counts(&ws).into_iter().enumerate() {
                    cx.child(cx.op(), vec![v.str("piece"), v.int(q as i64), v.int(c)]);
                }
                self.runs[PLAN].fetch_add(1, Relaxed);
                Ok(ValueId::UNIT)
            }
            _ => {
                let q = usize::try_from(int(v, a[1])).unwrap();
                cx.publish(pc_field(v, q), a[2]);
                self.runs[PLAN + 1 + q].fetch_add(1, Relaxed);
                Ok(ValueId::UNIT)
            }
        }
    }
}

struct Threads(usize);

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> phitex_defuse::RunResult + Sync),
        out: &mut Vec<phitex_defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<phitex_defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0 {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Relaxed);
                        if i >= n {
                            break;
                        }
                        let _ = slots[i].set(run(i));
                    }
                });
            }
        });
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("every job ran")));
    }
}

const SCHEDS: [&str; 7] = ["seq", "spec", "random", "par", "pred1", "pred8", "par8"];

fn eval(eng: &mut Engine, sched: &str) -> Result<Report, String> {
    match sched {
        "seq" => eng.evaluate_sequential(),
        "spec" => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut phitex_defuse::Sequential,
        ),
        "random" => eng.evaluate(&mut phitex_defuse::RandomScheduler::new(7, true), &mut phitex_defuse::Sequential),
        "pred1" => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 256),
            &mut Threads(1),
        ),
        "pred8" => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 256),
            &mut Threads(8),
        ),
        "par8" => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut Threads(8),
        ),
        _ => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut Threads(4),
        ),
    }
    .map_err(|e| {
        let s = format!("{e:?}");
        s.chars().take(600).collect()
    })
}

struct Client {
    eng: Engine,
    toy: Arc<Toy>,
    op: OpId,
    items: Vec<NodeId>,
    plan: NodeId,
    env: BTreeMap<String, i64>,
    answered: Arc<Mutex<Vec<(String, i64)>>>,
    declared: BTreeSet<usize>,
}

/// One client evaluation step as ours runs it: a fresh provider, evaluate,
/// mirror the provider's answers, and when the evaluation asked for
/// anything, declare and supply it (the full map) and evaluate again.
/// Returns each evaluation's report.
fn settle(c: &mut Client, sched: &str) -> Result<Vec<Report>, String> {
    let mut out = Vec::new();
    loop {
        let log = Arc::clone(&c.answered);
        c.eng.set_env_provider(Box::new(move |v, f| {
            let name = v.as_str(f.0)?;
            let rest = name.strip_prefix("file:")?;
            let mut it = rest.split(':');
            let i: usize = it.next()?.parse().ok()?;
            let e: usize = it.next()?.parse().ok()?;
            let x = env_val(i, e);
            log.lock().unwrap().push((name.clone(), x));
            Some(Some(v.int(x)))
        }));
        let r = eval(&mut c.eng, sched)?;
        for (n, x) in c.answered.lock().unwrap().drain(..) {
            c.env.insert(n, x);
        }
        let v = c.eng.values();
        let mut asked = false;
        let waits: Vec<String> = r.env_waits.iter().filter_map(|&(_, f)| v.as_str(f.0)).collect();
        for name in waits {
            if let Some(d) = name.strip_prefix("decl:").and_then(|d| d.parse::<usize>().ok()) {
                if c.declared.insert(d) {
                    let (f, x) = (lbx_field(c.eng.values(), d), c.eng.values().int(lbx_init(d)));
                    c.eng.declare_loopback(f, x);
                    c.env.insert(name, 0);
                    asked = true;
                }
            }
        }
        out.push(r);
        if !asked {
            break;
        }
        supply(c);
    }
    Ok(out)
}

/// The full environment map and absent set, as our client supplies them.
fn supply(c: &mut Client) {
    let v = c.eng.values();
    let env: Vec<(Field, ValueId)> = c.env.iter().map(|(n, &x)| (v.field(n), v.int(x))).collect();
    c.eng.set_environment(env);
    c.eng.set_environment_absent(Vec::<Field>::new());
}

/// The logical node an evaluation error names.
fn who(c: &Client, e: &str) -> String {
    let named = |n: NodeId| e.contains(&format!("{n:?}"));
    if let Some(i) = c.items.iter().position(|&n| named(n)) {
        let it = &c.toy.prog.items[i];
        return format!(
            "item{i} (reads keys {:?}, pieces {:?}, publishes keys {:?})",
            it.reads, it.pcs, it.defs
        );
    }
    if named(c.plan) {
        return "plan".to_owned();
    }
    "a piece or unknown".to_owned()
}

/// The nodes with the most completed runs so far.
fn top_runs(t: &Toy) -> String {
    let mut xs: Vec<(u64, usize)> = t.runs.iter().enumerate().map(|(i, x)| (x.load(Relaxed), i)).collect();
    xs.sort_unstable_by(|a, b| b.cmp(a));
    xs.iter()
        .take(5)
        .map(|&(r, i)| format!("#{i}:{r}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn take(t: &Toy) -> Vec<u64> {
    t.runs.iter().map(|x| x.swap(0, Relaxed)).collect()
}

fn line(r: &Report) -> String {
    format!(
        "runs={} nodes_run={} stops={} rounds={} env_provided={} rcp={}",
        r.runs, r.nodes_run, r.stops, r.rounds, r.env_provided, r.realised_critical_path
    )
}

/// Rerun causes by kind: the logical nodes per kind (items and the
/// planner by id; anything else as `other`).
fn causes(c: &Client, rs: &[Report]) -> BTreeMap<String, BTreeSet<String>> {
    let mut m: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in rs {
        for (n, why) in &r.rerun_causes {
            let k = format!("{why:?}");
            let k = k.split([' ', '(', '{']).next().unwrap_or("").to_owned();
            let who = if let Some(i) = c.items.iter().position(|x| x == n) {
                format!("item{i}")
            } else if *n == c.plan {
                "plan".to_owned()
            } else {
                "other".to_owned()
            };
            m.entry(k).or_default().insert(who);
        }
    }
    m
}

fn diff_runs(got: &[u64], want: &[u64]) -> String {
    let name = |i: usize| {
        if i < N {
            format!("item{i}")
        } else if i == PLAN {
            "plan".to_owned()
        } else {
            format!("piece{}", i - PLAN - 1)
        }
    };
    let d: Vec<String> = (0..NODES)
        .filter(|&i| got[i] != want[i])
        .map(|i| format!("{} {}!={}", name(i), got[i], want[i]))
        .collect();
    let n = d.len();
    let mut s: String = d.into_iter().take(12).collect::<Vec<_>>().join(", ");
    if n > 12 {
        s.push_str(&format!(", ... ({n} nodes)"));
    }
    s
}

#[test]
fn toy_d_loops_env() {
    let p = Arc::new(prog());
    let waiting: BTreeSet<usize> = DECL.iter().copied().collect();
    let a0: Vec<i64> = (0..N).map(|i| 10 + i as i64).collect();
    let w0: Vec<i64> = (0..N).map(|i| 1 + (i % 7) as i64).collect();
    // the previous program, whose fixpoint seeds the loop fields
    let (mut ap, mut wp) = (a0.clone(), w0.clone());
    for i in [3, 17, 60, 99, 140, 181] {
        ap[i] += 1000;
    }
    for i in [8, 77, 160] {
        wp[i] += 5;
    }
    let seeds = fixpoint(&p, &ap, &wp);
    let chains = p.items.iter().map(|it| it.reads.len()).sum::<usize>();
    println!(
        "program: {N} items, {} loop keys, {chains} key reads, {} piece reads, {} environment reads, {} waiting items",
        p.keys,
        p.items.iter().map(|it| it.pcs.len()).sum::<usize>(),
        p.items.iter().map(|it| it.envs.len()).sum::<usize>(),
        DECL.len()
    );
    let mut fails: Vec<String> = Vec::new();
    for sched in SCHEDS {
        let mut fail = |s: String| {
            println!("FAIL {sched}: {s}");
            fails.push(format!("{sched}: {s}"));
        };
        // ---- build
        let mut eng = Engine::new();
        let toy = Arc::new(Toy {
            prog: Arc::clone(&p),
            runs: (0..NODES).map(|_| AtomicU64::new(0)).collect(),
        });
        let op = eng.register(toy.clone());
        let wf = eng.values().field("w");
        eng.declare_accumulator(wf, Arc::new(phitex_defuse::SeqMonoid));
        for k in 0..p.keys {
            let (f, x) = (lp_field(eng.values(), k), eng.values().int(seeds.0[p.lp(k)]));
            eng.declare_loopback(f, x);
        }
        for q in 0..PIECES {
            let (f, x) = (pc_field(eng.values(), q), eng.values().int(seeds.0[p.pc(q)]));
            eng.declare_loopback(f, x);
        }
        eng.set_loop_start(LoopStart::Previous);
        let u = eng.new_unit();
        let mut items = Vec::new();
        for i in 0..N {
            let v = eng.values();
            let args = vec![v.str("item"), v.int(i as i64), v.int(a0[i]), v.int(w0[i])];
            items.push(eng.push(u, op, args));
        }
        let pa = vec![eng.values().str("plan")];
        let plan = eng.push(u, op, pa);
        let mut c = Client {
            eng,
            toy,
            op,
            items,
            plan,
            env: BTreeMap::new(),
            answered: Arc::new(Mutex::new(Vec::new())),
            declared: BTreeSet::new(),
        };
        // ---- the oracle, cold: evaluation 1 without the waiting items,
        // evaluation 2 with them
        let mut st = State {
            loops: seeds.clone(),
            last: vec![None; N],
            counts: None,
            outs: vec![None; N],
        };
        for d in 0..DECL.len() {
            st.loops.0[p.lbx(d)] = lbx_init(d);
        }
        let s1 = simulate(&p, &mut st, &a0, &w0, None, &waiting);
        let s2 = simulate(&p, &mut st, &a0, &w0, None, &BTreeSet::new());
        let rs = match settle(&mut c, sched) {
            Ok(rs) => rs,
            Err(e) => {
                fail(format!(
                    "cold: evaluation error {e} (node: {}; client runs per node so far: {})",
                    who(&c, &e),
                    top_runs(&c.toy)
                ));
                continue;
            }
        };
        let got = take(&c.toy);
        let want: Vec<u64> = (0..NODES).map(|i| s1.runs[i] + s2.runs[i]).collect();
        for (k, r) in rs.iter().enumerate() {
            println!("{sched} cold eval {}: {}", k + 1, line(r));
        }
        println!(
            "{sched} cold: client runs {} (oracle {}), oracle rounds {} + {}, causes {:?}",
            got.iter().sum::<u64>(),
            want.iter().sum::<u64>(),
            s1.rounds,
            s2.rounds,
            causes(&c, &rs)
                .iter()
                .map(|(k, s)| (k.clone(), s.len()))
                .collect::<BTreeMap<_, _>>()
        );
        if got != want {
            fail(format!("cold: per-node runs differ: {}", diff_runs(&got, &want)));
        }
        if rs.len() != 2 {
            fail(format!("cold: {} evaluations, want 2", rs.len()));
        } else {
            if rs[0].rounds != s1.rounds {
                fail(format!(
                    "cold: evaluation 1 took {} loop rounds, the seeds' fixpoint depth is {}",
                    rs[0].rounds, s1.rounds
                ));
            }
            if rs[1].rounds != 1 {
                fail(format!("cold: evaluation 2 took {} loop rounds, want 1", rs[1].rounds));
            }
        }
        let outs: Vec<Option<i64>> = c
            .items
            .iter()
            .map(|&n| c.eng.output(n).and_then(|x| c.eng.values().as_int(x)))
            .collect();
        if outs != st.outs {
            fail(format!(
                "cold: {} item outputs differ from the fixpoint",
                (0..N).filter(|&i| outs[i] != st.outs[i]).count()
            ));
        }
        // ---- edits, one value each
        let (mut a, mut w) = (a0.clone(), w0.clone());
        // the item with the most transitive readers of its keys
        let reach = |i: usize| -> usize {
            let mut seen = BTreeSet::from([i]);
            let mut todo = vec![i];
            while let Some(j) = todo.pop() {
                for (r, it) in p.items.iter().enumerate() {
                    if it.reads.iter().any(|k| p.items[j].defs.contains(k)) && seen.insert(r) {
                        todo.push(r);
                    }
                }
            }
            seen.len()
        };
        let deep = (0..N).max_by_key(|&i| reach(i)).unwrap();
        let read_piece = p.items.iter().find_map(|it| it.pcs.first().copied()).unwrap();
        let quiet = (0..N)
            .find(|&i| p.items[i].defs.is_empty() && p.items[i].decl.is_none())
            .unwrap();
        let edits: [(&str, usize, bool); 4] = [
            ("a of the item with the deepest readers", deep, true),
            ("w of an item in a piece that is read", read_piece * PER_PIECE, false),
            ("a of an item nobody reads", quiet, true),
            ("a of the same item back", deep, true),
        ];
        for (k, &(what, i, is_a)) in edits.iter().enumerate() {
            let name = format!("edit {} ({what}, item {i})", k + 1);
            if is_a {
                a[i] = if k == 3 { a0[i] } else { a[i] + 1 };
            } else {
                w[i] += 3;
            }
            let v = c.eng.values();
            let args = vec![v.str("item"), v.int(i as i64), v.int(a[i]), v.int(w[i])];
            c.eng.replace(c.items[i], c.op, args);
            // as our client: the re-made nodes' loop fields declared again
            // (same initial value), the full environment again
            for &d in &c.declared.clone() {
                let (f, x) = (lbx_field(c.eng.values(), d), c.eng.values().int(lbx_init(d)));
                c.eng.declare_loopback(f, x);
            }
            supply(&mut c);
            let s = simulate(&p, &mut st, &a, &w, Some(i), &BTreeSet::new());
            let rs = match settle(&mut c, sched) {
                Ok(rs) => rs,
                Err(e) => {
                    fail(format!(
                        "{name}: evaluation error {e} (node: {}; client runs: {})",
                        who(&c, &e),
                        top_runs(&c.toy)
                    ));
                    break;
                }
            };
            let got = take(&c.toy);
            let cs = causes(&c, &rs);
            for r in &rs {
                println!("{sched} {name}: {}", line(r));
            }
            println!(
                "{sched} {name}: client runs {} (oracle {}), oracle rounds {}, loop readers {}, range {}, causes {:?}",
                got.iter().sum::<u64>(),
                s.runs.iter().sum::<u64>(),
                s.rounds,
                s.loop_readers.len(),
                s.range,
                cs.iter().map(|(k, s)| (k.clone(), s.len())).collect::<BTreeMap<_, _>>()
            );
            if got != s.runs {
                fail(format!("{name}: per-node runs differ: {}", diff_runs(&got, &s.runs)));
            }
            if rs.len() != 1 {
                fail(format!("{name}: {} evaluations, want 1", rs.len()));
            }
            let rounds: u64 = rs.iter().map(|r| r.rounds).sum();
            if rounds != s.rounds {
                fail(format!("{name}: {rounds} loop rounds, the oracle's {}", s.rounds));
            }
            if let Some(e) = cs.get("Environment") {
                fail(format!(
                    "{name}: {} nodes re-ran for the environment (want none): {:?}",
                    e.len(),
                    e.iter().take(8).collect::<Vec<_>>()
                ));
            }
            let want_loop: BTreeSet<String> = s.loop_readers.iter().map(|i| format!("item{i}")).collect();
            let got_loop = cs.get("Loop").cloned().unwrap_or_default();
            if got_loop != want_loop {
                let extra: Vec<_> = got_loop.difference(&want_loop).take(8).collect();
                let missing: Vec<_> = want_loop.difference(&got_loop).take(8).collect();
                fail(format!(
                    "{name}: Loop re-runs on {} nodes, want {} (extra {extra:?}, missing {missing:?})",
                    got_loop.len(),
                    want_loop.len()
                ));
            }
            let got_range = cs.get("Range").cloned().unwrap_or_default();
            let want_range: BTreeSet<String> = if s.range {
                BTreeSet::from(["plan".to_owned()])
            } else {
                BTreeSet::new()
            };
            if got_range != want_range {
                fail(format!("{name}: Range re-runs on {got_range:?}, want {want_range:?}"));
            }
            let outs: Vec<Option<i64>> = c
                .items
                .iter()
                .map(|&n| c.eng.output(n).and_then(|x| c.eng.values().as_int(x)))
                .collect();
            if outs != st.outs {
                fail(format!(
                    "{name}: {} item outputs differ from the fixpoint",
                    (0..N).filter(|&i| outs[i] != st.outs[i]).count()
                ));
            }
        }
    }
    println!("{} failures", fails.len());
    assert!(fails.is_empty(), "{} failures (see FAIL lines)", fails.len());
}
