//! The page chain (PhiTeX `docs/spec-page-chain.md` §4.3): pieces
//! typeset up front, one scan step per page fire. Scan `p`, placed after
//! finish `p - 1`, reads the material finish `p - 1` gave back (`rtn`),
//! then the profiles of the pieces after it, in order, until the page is
//! full at piece `d_p`; it places finish `p` right after `d_p`, and scan
//! `p + 1` after that. Finish `p` defines `rtn` and a page-state field
//! `g` that some pieces read (their height follows it): the pieces after
//! `d_p` that read it are re-asked for `g` alone.
//!
//! Two read patterns: (a) `acc_next`, the next piece's profile one at a
//! time, nothing read past `d_p`; (b) `acc_fold` over a window of `W`
//! pieces, reading past `d_p`, so a piece there that reads `g` refutes
//! the scan that placed the finish defining it (a rerun that decides the
//! same). Every configuration must give the pages a sequential page
//! builder gives, cold and after an edit; the counts are printed (run
//! the binary with and without `CLEAN_LANE=1` for the lane).

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, RwLock};

use defuse::{Ctx, Engine, Field, Locality, Monoid, NodeId, OpId, Operation, Stop, ValueId, Values};

/// Pieces, the page height, a piece's and a finisher's work.
const N: usize = 1200;
const H: i64 = 40;
const PIECE_WORK: u64 = 40_000;
const FINISH_WORK: u64 = 10_000;
const W: usize = 12;

/// Lists, concatenated.
struct Cat;

impl Monoid for Cat {
    fn empty(&self, v: &Values) -> ValueId {
        v.list(Vec::new())
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        let mut xs = v.with_list(a, <[ValueId]>::to_vec).unwrap_or_default();
        xs.extend(v.with_list(b, <[ValueId]>::to_vec).unwrap_or_default());
        v.list(xs)
    }
}

/// Piece `i`'s height and whether it reads `g` (one in `PAGE_CHAIN_G`,
/// default 50: rare, as PhiTeX's P0 found).
fn piece(i: usize, bump: i64) -> (i64, bool) {
    static G: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let every = *G.get_or_init(|| std::env::var("PAGE_CHAIN_G").ok().and_then(|x| x.parse().ok()).unwrap_or(50));
    (3 + i64::try_from((i * 7 + i / 5) % 6).unwrap() + bump, i % every == 7 % every)
}

fn busy(n: u64) -> u64 {
    let mut x = 0u64;
    for i in 0..n {
        x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(i));
    }
    x
}

struct Ops {
    op: RwLock<Option<OpId>>,
    /// The pieces, and each one's index.
    ids: RwLock<Vec<NodeId>>,
    index: RwLock<HashMap<u64, usize>>,
    /// Finish nodes predict their definitions (Q2).
    predict: AtomicBool,
    runs: [AtomicU64; 3],
    /// Scan runs' time (ns) and the pieces they read (Q1's cost per
    /// extension).
    scan_ns: AtomicU64,
    reads: AtomicU64,
}

struct F {
    vprof: Field,
    pages: Field,
    rtn: Field,
    g: Field,
}

fn fields(v: &Values) -> F {
    F {
        vprof: v.field("vprof"),
        pages: v.field("pages"),
        rtn: v.field("rtn"),
        g: v.field("g"),
    }
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn int(v: &Values, x: ValueId) -> i64 {
    v.as_int(x).unwrap()
}

impl Ops {
    /// Scan `p` decided at piece `d` with `over` left over: finish `p`,
    /// then scan `p + 1`, right after `d`.
    fn place(&self, cx: &mut Ctx<'_>, p: i64, d: NodeId, over: i64, mode: ValueId) {
        let v = cx.values();
        let op = self.op.read().unwrap().expect("registered");
        let raw = i64::try_from(d.raw()).unwrap();
        cx.follow_after_any(d, op, vec![v.str("finish"), v.int(p), v.int(raw), v.int(over)]);
        cx.follow_after_any(d, op, vec![v.str("scan"), v.int(p + 1), v.int(raw), mode]);
    }

    /// The last page: finish `p`, nothing given back, no next scan.
    fn last(&self, cx: &mut Ctx<'_>, p: i64) {
        let v = cx.values();
        let op = self.op.read().unwrap().expect("registered");
        cx.follow(op, vec![v.str("finish"), v.int(p), v.int(-1), v.int(0)]);
    }

    fn scan(&self, cx: &mut Ctx<'_>, a: &[ValueId]) -> Result<ValueId, Stop> {
        let v = cx.values();
        let f = fields(v);
        let p = int(v, a[1]);
        let after = u64::try_from(int(v, a[2])).ok().map(NodeId::from_raw);
        let window = v.as_str(a[3]).unwrap_or_default() == "b";
        let mut total = if p == 0 { 0 } else { cx.use_field(f.rtn)?.map_or(0, |x| int(v, x)) };
        if window {
            // (b): windows of W pieces, read past the decision.
            let ids = self.ids.read().unwrap().clone();
            let mut from = after;
            let mut k = after.map_or(0, |x| self.index.read().unwrap()[&x.raw()] + 1);
            while k < ids.len() {
                let end = (k + W).min(ids.len()) - 1;
                let prof = cx.acc_fold(f.vprof, from, Some(ids[end]))?;
                let hs: Vec<i64> = v.with_list(prof, |xs| xs.iter().map(|&x| int(v, x)).collect()).unwrap();
                for (j, h) in (k..=end).zip(hs) {
                    total += h;
                    self.reads.fetch_add(1, Relaxed);
                    if total >= H {
                        self.place(cx, p, ids[j], total - H, a[3]);
                        return Ok(v.int(p));
                    }
                }
                from = Some(ids[end]);
                k = end + 1;
            }
        } else {
            // (a): the next piece's profile at a time, up to the decision.
            let mut cur = after;
            while let Some((x, prof)) = cx.acc_next(f.vprof, cur)? {
                total += v.with_list(prof, |xs| int(v, xs[0])).unwrap();
                self.reads.fetch_add(1, Relaxed);
                cur = Some(x);
                if total >= H {
                    self.place(cx, p, x, total - H, a[3]);
                    return Ok(v.int(p));
                }
            }
        }
        if total > 0 || p == 0 {
            self.last(cx, p);
        }
        Ok(v.int(p))
    }
}

impl Operation for Ops {
    fn name(&self) -> &'static str {
        "page"
    }
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let f = fields(v);
        Some(if tag(v, a) == "finish" { vec![(f.rtn, Locality::Global), (f.g, Locality::Global)] } else { Vec::new() })
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let f = fields(v);
        match tag(v, a).as_str() {
            "finish" => Some(vec![(f.pages, v.list(vec![v.list(vec![a[1], a[2]])]))]),
            "scan" => Some(Vec::new()),
            _ => None,
        }
    }
    fn contribs_complete(&self, a: &[ValueId], v: &Values) -> bool {
        tag(v, a) == "finish"
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let f = fields(v);
        (tag(v, a) == "finish" && self.predict.load(Relaxed)).then(|| vec![(f.rtn, a[3]), (f.g, a[1])])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let f = fields(v);
        match tag(v, &a).as_str() {
            "piece" => {
                self.runs[0].fetch_add(1, Relaxed);
                let (h, reads_g) = (int(v, a[1]), int(v, a[2]) != 0);
                let g = if reads_g { cx.use_field(f.g)?.map_or(0, |x| int(v, x)) } else { 0 };
                let h = h + g.rem_euclid(2);
                busy(PIECE_WORK);
                cx.contribute(f.vprof, v.list(vec![v.int(h)]));
                Ok(v.int(h))
            }
            "finish" => {
                self.runs[1].fetch_add(1, Relaxed);
                busy(FINISH_WORK);
                cx.define(f.rtn, a[3]);
                cx.define(f.g, a[1]);
                cx.contribute(f.pages, v.list(vec![v.list(vec![a[1], a[2]])]));
                Ok(ValueId::UNIT)
            }
            _ => {
                self.runs[2].fetch_add(1, Relaxed);
                let t = std::time::Instant::now();
                let r = self.scan(cx, &a);
                self.scan_ns.fetch_add(u64::try_from(t.elapsed().as_nanos()).unwrap(), Relaxed);
                r
            }
        }
    }
}

struct Threads(usize);

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> defuse::RunResult + Sync),
        out: &mut Vec<defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0.min(n) {
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
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("ran")));
    }
}

/// The pages a sequential page builder gives: each fire's piece index
/// (-1: the last page, at the end).
fn reference(bumped: Option<usize>) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let (mut total, mut g, mut p) = (0, 0, 0);
    for i in 0..N {
        let (h, reads_g) = piece(i, i64::from(Some(i) == bumped) * 5);
        total += h + if reads_g { g % 2 } else { 0 };
        if total >= H {
            out.push((p, i64::try_from(i).unwrap()));
            total -= H;
            g = p;
            p += 1;
        }
    }
    if total > 0 || p == 0 {
        out.push((p, -1));
    }
    out
}

fn build(mode: &str, predict: bool) -> (Engine, Arc<Ops>, defuse::UnitId) {
    let mut eng = Engine::new();
    let f = fields(eng.values());
    eng.declare_accumulator(f.vprof, Arc::new(Cat));
    eng.declare_accumulator(f.pages, Arc::new(Cat));
    let ops = Arc::new(Ops {
        op: RwLock::new(None),
        ids: RwLock::new(Vec::new()),
        index: RwLock::new(HashMap::new()),
        predict: AtomicBool::new(predict),
        runs: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
        scan_ns: AtomicU64::new(0),
        reads: AtomicU64::new(0),
    });
    let op = eng.register(ops.clone());
    *ops.op.write().unwrap() = Some(op);
    let u = eng.new_unit();
    let v = eng.values();
    let scan0 = vec![v.str("scan"), v.int(0), v.int(-1), v.str(mode)];
    eng.push(u, op, scan0);
    for i in 0..N {
        let (h, reads_g) = piece(i, 0);
        let v = eng.values();
        let a = vec![v.str("piece"), v.int(h), v.int(i64::from(reads_g))];
        let x = eng.push(u, op, a);
        ops.ids.write().unwrap().push(x);
        ops.index.write().unwrap().insert(x.raw(), i);
    }
    (eng, ops, u)
}

/// The pages the engine's finish nodes made, in text order, as piece
/// indexes.
fn pages(eng: &Engine, ops: &Ops, u: defuse::UnitId) -> Vec<(i64, i64)> {
    let v = eng.values();
    let all = eng.acc_value(u, fields(v).pages).expect("pages");
    let index = ops.index.read().unwrap();
    v.with_list(all, |xs| {
        xs.iter()
            .map(|&x| {
                v.with_list(x, |pd| {
                    let d = int(v, pd[1]);
                    let d = u64::try_from(d).map_or(-1, |raw| i64::try_from(index[&raw]).unwrap());
                    (int(v, pd[0]), d)
                })
                .unwrap()
            })
            .collect()
    })
    .unwrap()
}

#[test]
fn the_page_chain_gives_the_sequential_pages_with_each_read_pattern() {
    let want = reference(None);
    let fires = want.len();
    let lane = std::env::var_os("CLEAN_LANE").is_some();
    eprintln!("page chain: {N} pieces, {fires} fires, lane {}", if lane { "on" } else { "off" });
    for mode in ["a", "b"] {
        for predict in [false, true] {
            for threads in [1usize, 4, 8] {
                let (mut eng, ops, u) = build(mode, predict);
                let t = std::time::Instant::now();
                let r = if threads == 1 {
                    eng.evaluate_sequential()
                } else {
                    eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut Threads(threads))
                }
                .expect("evaluate");
                let wall = t.elapsed();
                assert_eq!(pages(&eng, &ops, u), want, "mode {mode}, predict {predict}, threads {threads}");
                let runs: Vec<u64> = ops.runs.iter().map(|x| x.load(Relaxed)).collect();
                eprintln!(
                    "  {mode} predict {predict:5} j{threads}: {:7.1} ms, steps {:5}, runs piece/finish/scan {:5}/{:4}/{:4}, path {:5} (bar {}), critical time {:6.1} ms, refuted {}",
                    wall.as_secs_f64() * 1e3,
                    r.steps,
                    runs[0],
                    runs[1],
                    runs[2],
                    r.realised_critical_path,
                    2 * fires + 10,
                    r.realised_critical_time.as_secs_f64() * 1e3,
                    r.mispredictions,
                );
                let (ns, n) = (ops.scan_ns.load(Relaxed), ops.reads.load(Relaxed).max(1));
                eprintln!("    scans: {:.2} us per piece read ({n} reads)", ns as f64 / n as f64 / 1e3);
                if std::env::var_os("PAGE_CHAIN_DETAIL").is_some() {
                    eprintln!("    widths {:?}, stops {}, max_ready {}, speculations {}, refuted_by_field {:?}", r.applied_widths, r.stops, r.max_ready, r.speculations, r.refuted_by_field);
                }
                // A one-piece edit: its page and the later ones move.
                let k = N / 2;
                let (h, reads_g) = piece(k, 5);
                let x = ops.ids.read().unwrap()[k];
                let op = ops.op.read().unwrap().expect("registered");
                let v = eng.values();
                let a = vec![v.str("piece"), v.int(h), v.int(i64::from(reads_g))];
                eng.replace(x, op, a);
                let before: Vec<u64> = ops.runs.iter().map(|x| x.load(Relaxed)).collect();
                let t = std::time::Instant::now();
                let r = if threads == 1 {
                    eng.evaluate_sequential()
                } else {
                    eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut Threads(threads))
                }
                .expect("edit");
                let wall = t.elapsed();
                assert_eq!(pages(&eng, &ops, u), reference(Some(k)), "edit: mode {mode}, predict {predict}, threads {threads}");
                let runs: Vec<u64> = ops.runs.iter().zip(&before).map(|(x, b)| x.load(Relaxed) - b).collect();
                eprintln!(
                    "    edit: {:6.1} ms, steps {:4}, runs piece/finish/scan {:4}/{:4}/{:4}",
                    wall.as_secs_f64() * 1e3,
                    r.steps,
                    runs[0],
                    runs[1],
                    runs[2],
                );
            }
        }
    }
}
