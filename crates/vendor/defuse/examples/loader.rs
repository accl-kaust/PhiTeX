//! The loader chain: `n` nodes in text order, each reading `r` fields and
//! defining `d` fields, with definition sets unknown in advance
//! (`MayDefine::Any`); a fraction `h` of the reads hit an earlier node's
//! definitions. Reports wall time, runs per node and re-runs at 1 and 12
//! workers.
//!
//! `loader [n] [r] [d] [h] [seed]`, defaults 40 32 1000 0.1 1.
//!
//! `loader chain [n] [work_us] [depth] [r]`, defaults 6588 600 4 3: the
//! client's preamble shape. Operation `k` spins `work_us` microseconds,
//! defines `n{k}.0`, and reads `r` fields: the previous operation's when
//! it is not the first of its chain of `depth`, the rest from earlier
//! chain heads (or fields nobody defines); a head reads only fields
//! nobody defines.
//!
//! `loader client [--narrow] [--declared] [n] [work_us] [reads]`, defaults 5204 600 12: the
//! client's text operations as Phi describes them. Each declares a
//! bounded definition set (prefixes `t<k>.` and `sty.`), its reads (a
//! hint), a static claim of none, spins `work_us`, reads its fields
//! (the previous operation's `t<k-1>.a` within chains of 4, earlier `sty.<j>`, and fields
//! nobody defines), defines `t<k>.a` (every tenth also `sty.<k/10>`),
//! contributes 12 items to `items`, asks its scope kind and its first
//! following sibling. A plan operation at the end folds `items`. Reports wall, runs, stops,
//! gated, and (profiling builds) the engine's walks and finish time, at
//! 1 and 12 workers. `--declared`: each operation declares its exact
//! definition set (`Operation::defs`) instead.
//! (A custom operation: the toy language has no statement with an
//! unbounded definition set of this size.)

#![allow(clippy::many_single_char_names, clippy::cast_possible_truncation)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::Threads;
use defuse::{
    Ctx, Engine, Executor, Field, Greedy, Locality, MayDefine, Operation, Rng, RunResult, Speculation, Stop, ValueId,
    Values,
};

/// Every job on its own claim of `T` scoped threads, however few the
/// jobs (each run here is long).
struct LongThreads(usize);

impl Executor for LongThreads {
    fn parallelism(&self) -> usize {
        self.0.max(1)
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>) {
        let next = AtomicU64::new(0);
        let slots: Vec<std::sync::OnceLock<RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0.clamp(1, n.max(1)) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed) as usize;
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

/// `[k, d, reads...]`: reads the named fields, defines `n{k}.{i}` for
/// `i < d` to a hash of what it read.
struct Loader;

impl Operation for Loader {
    fn name(&self) -> &'static str {
        "loader"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let k = v.as_int(a[0]).unwrap_or(0);
        let d = v.as_int(a[1]).unwrap_or(0);
        let work = std::time::Duration::from_micros(v.as_int(a[2]).unwrap_or(0).unsigned_abs());
        let mut h: u64 = 0x5eed;
        for &f in &a[3..] {
            let x = cx.use_field(Field(f))?;
            h = defuse::hash_words(h, &[x.map_or(0, ValueId::raw)]);
        }
        let t0 = std::time::Instant::now();
        while t0.elapsed() < work {
            std::hint::spin_loop();
        }
        // Work proportional to the definitions made.
        for i in 0..d {
            let x = v.int(i64::from_ne_bytes(defuse::hash_words(h, &[i.unsigned_abs()]).to_ne_bytes()) >> 4);
            cx.define_with(v.field(&format!("n{k}.{i}")), x, Locality::Global);
        }
        Ok(v.int(i64::from_ne_bytes(h.to_ne_bytes()) >> 4))
    }
}

fn build(n: i64, r: usize, d: i64, hit: f64, seed: u64) -> (Engine, Vec<defuse::NodeId>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Loader));
    let u = eng.new_unit();
    let mut rng = Rng::new(seed);
    let mut ids = Vec::new();
    for k in 0..n {
        let v = eng.values();
        let mut args = vec![v.int(k), v.int(d), v.int(0)];
        for j in 0..r {
            #[allow(clippy::cast_precision_loss)]
            let hits = k > 0 && (rng.below(1_000_000) as f64) < hit * 1e6;
            let name = if hits {
                let p = rng.below(usize::try_from(k).unwrap_or(1));
                format!("n{p}.{}", rng.below(usize::try_from(d).unwrap_or(1)))
            } else {
                format!("env{k}.{j}")
            };
            args.push(v.field(&name).0);
        }
        ids.push(eng.push(u, op, args));
    }
    (eng, ids)
}

/// `["t", k, work, reads...]` or `["plan"]`: the client shape.
/// `Client(true)` declares each operation's exact definition set.
struct Client(bool);

impl Operation for Client {
    fn name(&self) -> &'static str {
        "client"
    }
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        if !self.0 {
            return None;
        }
        if a.len() < 2 {
            return Some(Vec::new());
        }
        let k = v.as_int(a[1]).unwrap_or(0);
        let mut d = vec![(v.field(&format!("t{k}.a")), Locality::Global)];
        if k % 10 == 0 {
            d.push((v.field(&format!("sty.{}", k / 10)), Locality::Global));
        }
        Some(d)
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        if a.len() < 2 {
            return MayDefine::Nothing;
        }
        let k = v.as_int(a[1]).unwrap_or(0);
        MayDefine::Prefixes(vec![format!("t{k}."), "sty.".into()])
    }
    fn reads(&self, a: &[ValueId], _: &Values) -> Option<Vec<Field>> {
        (a.len() > 3).then(|| a[3..].iter().map(|&x| Field(x)).collect())
    }
    fn claim(
        &self,
        a: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (defuse::OpId, Vec<ValueId>)>,
    ) -> defuse::Claim {
        if a.len() > 1 && std::env::var("NOCLAIM").is_err() {
            defuse::Claim::Count(0)
        } else {
            defuse::Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if a.len() < 2 {
            let x = cx.acc_range(v.field("items"), None)?;
            return Ok(v.int(i64::try_from(v.seq_len(x)).unwrap_or(0)));
        }
        let k = v.as_int(a[1]).unwrap_or(0);
        let work = std::time::Duration::from_micros(v.as_int(a[2]).unwrap_or(0).unsigned_abs());
        let mut h: u64 = 0x5eed;
        for (i, &f) in a[3..].iter().enumerate() {
            let x = cx.use_field(Field(f));
            if i == 0 && NARROW.load(Ordering::Relaxed) {
                // After its first read, the operation knows its fields
                // (Phi's runs: a read decides the definition set).
                let mut fs = vec![v.field(&format!("t{k}.a"))];
                if k % 10 == 0 {
                    fs.push(v.field(&format!("sty.{}", k / 10)));
                }
                cx.narrow(MayDefine::Names(fs));
            }
            let x = x?;
            h = defuse::hash_words(h, &[x.map_or(0, ValueId::raw)]);
        }
        if std::env::var("NOKIND").is_err() {
            let _ = cx.scope_kind()?;
        }
        if std::env::var("NOFOLLOW").is_err() {
            let _ = cx.following(0);
        }
        if std::env::var("NOCLAIM").is_err() {
            cx.claim(0);
        }
        let t0 = std::time::Instant::now();
        while t0.elapsed() < work {
            std::hint::spin_loop();
        }
        let out = v.int(i64::from_ne_bytes(h.to_ne_bytes()) >> 4);
        cx.define_with(v.field(&format!("t{k}.a")), out, Locality::Global);
        if k % 10 == 0 {
            cx.define_with(v.field(&format!("sty.{}", k / 10)), out, Locality::Global);
        }
        let items = v.field("items");
        for i in 0..12 {
            cx.contribute(items, v.seq_unit(v.int(k * 12 + i)));
        }
        Ok(out)
    }
}

fn build_client(n: i64, work: i64, r: usize, declared: bool) -> (Engine, Vec<defuse::NodeId>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Client(declared)));
    let items = eng.values().field("items");
    eng.declare_accumulator(items, Arc::new(defuse::SeqMonoid));
    let u = eng.new_unit();
    let mut rng = Rng::new(11);
    let mut ids = Vec::new();
    for k in 0..n {
        let v = eng.values();
        let mut args = vec![v.str("t"), v.int(k), v.int(work)];
        for j in 0..r {
            let name = if j == 0 && k % 4 != 0 {
                format!("t{}.a", k - 1)
            } else if k >= 10 && j % 2 == 1 {
                format!("sty.{}", rng.below(usize::try_from(k / 10).unwrap_or(1)))
            } else {
                format!("env{k}.{j}")
            };
            args.push(v.field(&name).0);
        }
        ids.push(eng.push(u, op, args));
    }
    let v = eng.values();
    ids.push(eng.push(u, op, vec![v.str("plan")]));
    (eng, ids)
}

/// `loader client --narrow`: each text operation narrows its definition
/// set to the fields it defines after its first read.
static NARROW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn client_main(a: &[String]) {
    let narrow = a.iter().any(|s| s == "--narrow");
    let declared = a.iter().any(|s| s == "--declared");
    NARROW.store(narrow, Ordering::Relaxed);
    let a: Vec<String> = a
        .iter()
        .filter(|s| *s != "--narrow" && *s != "--declared")
        .cloned()
        .collect();
    let a = &a[..];
    let n: i64 = a.get(1).map_or(5204, |s| s.parse().unwrap());
    let work: i64 = a.get(2).map_or(600, |s| s.parse().unwrap());
    let r: usize = a.get(3).map_or(12, |s| s.parse().unwrap());
    let mut reference = None;
    // `THREADS=1,4,8,12` picks the worker counts (default 1 and 12).
    let ts: Vec<usize> = std::env::var("THREADS").ok().map_or_else(
        || vec![1, 12],
        |s| s.split(',').filter_map(|x| x.parse().ok()).collect(),
    );
    for t in ts {
        let (mut eng, ids) = build_client(n, work, r, declared);
        defuse::obs::reset();
        let t0 = std::time::Instant::now();
        let rep = eng
            .evaluate(
                &mut match std::env::var("SPEC").as_deref() {
                    Ok("off") => Greedy::default(),
                    Ok("predicted") => Greedy::speculative(Speculation::Predicted, usize::MAX),
                    _ => {
                        let mut g = Greedy::speculative(Speculation::All, usize::MAX);
                        if let Some(o) = std::env::var("OUTSTANDING").ok().and_then(|v| v.parse().ok()) {
                            g.outstanding = o;
                        }
                        g
                    }
                },
                &mut LongThreads(t),
            )
            .expect("evaluate");
        let wall = t0.elapsed();
        let outs: Vec<Option<ValueId>> = ids.iter().map(|&x| eng.output(x)).collect();
        match &reference {
            None => reference = Some(outs),
            Some(r) => assert_eq!(*r, outs, "results differ between worker counts"),
        }
        println!(
            "client{} n {n} work {work}us r {r} | {t:>2} workers: wall {wall:?}  runs {}  stops {}  gated {}  critical path {}",
            match (narrow, declared) {
                (true, true) => " --narrow --declared",
                (true, false) => " --narrow",
                (false, true) => " --declared",
                (false, false) => "",
            },
            rep.runs,
            rep.stops,
            rep.gated,
            rep.critical_path
        );
        println!(
            "  speculations {} confirmations {} mispredictions {} spec_stops {} shape {} raises {} steps {} refuted_by_field {:?} restarted_by {:?} waits {:?}",
            rep.speculations,
            rep.confirmations,
            rep.mispredictions,
            rep.spec_stops,
            rep.spec_shape_changes,
            rep.raises,
            rep.steps,
            rep.refuted_by_field
                .iter()
                .take(8)
                .map(|(f, c)| (format!("{:?}", eng.values().get(f.0)), *c))
                .collect::<Vec<_>>(),
            rep.restarted_by,
            rep.waits_by_kind
        );
        println!("  widths {:?} max_ready {}", rep.applied_widths, rep.max_ready);
        let mut causes: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for (_, c) in &rep.rerun_causes {
            let s = format!("{c:?}");
            *causes
                .entry(s.split([' ', '{', '(']).next().unwrap_or("").to_string())
                .or_default() += 1;
        }
        println!(
            "  refuted: nodes {} max {} fields {}  rerun causes {causes:?}",
            rep.refuted_nodes,
            rep.refutations_max,
            rep.refuted_by_field.iter().map(|x| x.1).sum::<u64>()
        );
        if std::env::var("CHAIN").is_ok() {
            for (id, h, c) in eng.critical_chain_heights().iter().take(12) {
                println!("  {id:?} h {h} {c:?}");
            }
        }
        if defuse::obs::enabled() {
            print!("probes ({t} workers):\n{}", defuse::obs::snapshot());
        }
    }
}

/// The client preamble shape (see the module comment).
fn build_chain(n: i64, work: i64, depth: i64, r: usize) -> (Engine, Vec<defuse::NodeId>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Loader));
    let u = eng.new_unit();
    let mut rng = Rng::new(7);
    let mut ids = Vec::new();
    for k in 0..n {
        let v = eng.values();
        let mut args = vec![v.int(k), v.int(1), v.int(work)];
        let pos = k % depth;
        for j in 0..r {
            let name = if j == 0 && pos > 0 {
                format!("n{}.0", k - 1)
            } else if pos > 0 && k >= depth && rng.below(2) == 0 {
                let heads = usize::try_from(k / depth).unwrap_or(1);
                format!("n{}.0", i64::try_from(rng.below(heads)).unwrap_or(0) * depth)
            } else {
                format!("env{k}.{j}")
            };
            args.push(v.field(&name).0);
        }
        ids.push(eng.push(u, op, args));
    }
    (eng, ids)
}

fn chain_main(a: &[String]) {
    let n: i64 = a.get(1).map_or(6588, |s| s.parse().unwrap());
    let work: i64 = a.get(2).map_or(600, |s| s.parse().unwrap());
    let depth: i64 = a.get(3).map_or(4, |s| s.parse().unwrap());
    let r: usize = a.get(4).map_or(3, |s| s.parse().unwrap());
    let mut reference = None;
    for t in [1usize, 12] {
        let (mut eng, ids) = build_chain(n, work, depth, r);
        defuse::obs::reset();
        let t0 = std::time::Instant::now();
        let rep = eng
            .evaluate(&mut Greedy::speculative(Speculation::All, usize::MAX), &mut Threads(t))
            .expect("evaluate");
        let wall = t0.elapsed();
        let outs: Vec<Option<ValueId>> = ids.iter().map(|&x| eng.output(x)).collect();
        match &reference {
            None => reference = Some(outs),
            Some(r) => assert_eq!(*r, outs, "results differ between worker counts"),
        }
        println!(
            "chain n {n} work {work}us depth {depth} r {r} | {t:>2} workers: wall {wall:?}  runs {}  stops {}  gated {}  critical path {}",
            rep.runs, rep.stops, rep.gated, rep.critical_path
        );
        if defuse::obs::enabled() {
            print!("probes ({t} workers):\n{}", defuse::obs::snapshot());
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.first().is_some_and(|s| s == "client") {
        client_main(&a);
        return;
    }
    if a.first().is_some_and(|s| s == "chain") {
        chain_main(&a);
        return;
    }
    let n: i64 = a.first().map_or(40, |s| s.parse().unwrap());
    let r: usize = a.get(1).map_or(32, |s| s.parse().unwrap());
    let d: i64 = a.get(2).map_or(1000, |s| s.parse().unwrap());
    let hit: f64 = a.get(3).map_or(0.1, |s| s.parse().unwrap());
    let seed: u64 = a.get(4).map_or(1, |s| s.parse().unwrap());
    let mut reference = None;
    for t in [1usize, 12] {
        let (mut eng, ids) = build(n, r, d, hit, seed);
        defuse::obs::reset();
        let t0 = std::time::Instant::now();
        let rep = eng
            .evaluate(&mut Greedy::speculative(Speculation::All, usize::MAX), &mut Threads(t))
            .expect("evaluate");
        let wall = t0.elapsed();
        let outs: Vec<Option<ValueId>> = ids.iter().map(|&x| eng.output(x)).collect();
        match &reference {
            None => reference = Some(outs),
            Some(r) => assert_eq!(*r, outs, "results differ between worker counts"),
        }
        #[allow(clippy::cast_precision_loss)]
        let per = rep.runs as f64 / n as f64;
        println!(
            "loader n {n} r {r} d {d} hit {hit} seed {seed} | {t:>2} workers: wall {wall:?}  runs {} ({per:.2}/node)  re-runs {}  stops {}  speculations {}  critical path {}",
            rep.runs,
            rep.runs.saturating_sub(u64::try_from(n).unwrap_or(0)),
            rep.stops,
            rep.speculations,
            rep.critical_path
        );
        if defuse::obs::enabled() {
            print!("probes ({t} workers):\n{}", defuse::obs::snapshot());
        }
    }
}
