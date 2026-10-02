//! The rebuild after one changed call in a 10k-call stub program, and
//! the cost per verified hit (`DESIGN.md` §7.17.2: about 100 ns).
//!
//! `cargo run --release -p partex-ssa --example bench --features std`
//! `[-- EDITS [profile]]`: `EDITS` rebuilds (40); `profile` skips the
//! plain builds, so a profiler sees the rebuilds. Then the cost of
//! `note_read` as the engine calls it (dense and hashed slots).

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use partex_ssa::stub::{Effect, Func, Stub, Val, source};
use partex_ssa::{Config, Cx, Loc, Machine, MapStore, Runtime, Version};

/// The engine's shape: slots in dense families (`Machine::dense`).
struct Dense;

/// The same slots, hashed.
struct Sparse;

impl Machine for Dense {
    type Addr = u32;
    type Val = Val;
    type Effect = Effect;
    type Func = Func;
    fn run<C: Cx<Self>>(&self, _: Func, _: &[Val], _: &mut C) -> Val {
        Val::Nil
    }
    fn dense(a: &u32) -> Option<(usize, usize)> {
        Some((0, *a as usize))
    }
}

impl Machine for Sparse {
    type Addr = u32;
    type Val = Val;
    type Effect = Effect;
    type Func = Func;
    fn run<C: Cx<Self>>(&self, _: Func, _: &[Val], _: &mut C) -> Val {
        Val::Nil
    }
}

/// `calls` calls, each reading `reads` times from `slots` distinct slots
/// (a paragraph reads the same `eqtb` entries over and over), with a
/// write every 64 reads; ns per `note_read`.
fn note_reads<M: Machine<Addr = u32, Val = Val, Func = Func>>(
    calls: u32,
    reads: u32,
    slots: u32,
) -> f64 {
    let mut rt = Runtime::<M>::new(Config::default());
    let st = MapStore::<M>::default();
    let addrs: Vec<Vec<Loc<u32>>> = (0..16)
        .map(|c| {
            (0..reads)
                .map(|k| Loc::State((k.wrapping_mul(2_654_435_761) >> 7) % slots + c * slots))
                .collect()
        })
        .collect();
    let mut best = f64::MAX;
    for round in 0..5 {
        rt.open_trip(0);
        let t = Instant::now();
        for c in 0..calls {
            rt.begin(Func::Para, vec![Version(u128::from(c + round * calls))]);
            for (k, loc) in addrs[(c % 16) as usize].iter().enumerate() {
                rt.note_read_with(loc, || Version(u128::from(*loc.addr())));
                if k % 64 == 0 {
                    rt.note_write(&(loc.addr() + 1));
                }
            }
            rt.end(&st, Val::Nil);
        }
        let dt = t.elapsed().as_nanos() as f64;
        rt.close_trip();
        best = best.min(dt / f64::from(calls * reads));
    }
    best
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let edits: usize = argv.get(1).and_then(|a| a.parse().ok()).unwrap_or(40);
    let profile = argv.get(2).is_some_and(|a| a == "profile");
    if argv.get(1).is_some_and(|a| a == "reads") {
        return reads_bench();
    }
    let paras = 2500;
    let mut lines = Vec::new();
    for p in 0..paras {
        if p > 0 {
            lines.push(String::new());
        }
        lines.push(format!("word{p} set x{} {} the quick brown", p % 7, p % 5));
    }
    let mut rt = Runtime::<Stub>::new(Config {
        keep: 2,
        ..Config::default()
    });
    let t = Instant::now();
    let _ = rt.build(&Stub, Func::Main, &[source(&lines)]);
    let cold = t.elapsed();
    let mut samples = Vec::new();
    let mut last = None;
    for k in 0..edits {
        let i = 2 * (k * 61 % paras);
        lines[i] = format!("{} edited{k}", lines[i]);
        let src = source(&lines);
        let h0 = rt.stats;
        let t = Instant::now();
        let out = rt.build(&Stub, Func::Main, &[src]);
        let dt = t.elapsed();
        let hits = rt.stats.hits - h0.hits;
        let misses = rt.stats.misses - h0.misses;
        let reads = rt.stats.reads_verified - h0.reads_verified;
        samples.push(dt.as_nanos());
        last = Some((hits, misses, reads, out.trips.len()));
    }
    samples.sort_unstable();
    let med = samples[samples.len() / 2];
    let min = samples[0];
    let (hits, misses, reads, trips) = last.expect("a sample");
    let calls = hits + misses;
    println!("program: {paras} paragraphs, {} lines", lines.len());
    println!("cold build: {:.2} ms", cold.as_secs_f64() * 1e3);
    println!(
        "rebuild after one changed line: median {:.1} us over {} runs; {calls} calls evaluated ({hits} hits, {misses} misses, {reads} reads verified, {trips} trip)",
        med as f64 / 1e3,
        samples.len()
    );
    println!(
        "per evaluated call: {:.0} ns (median), {:.0} ns (fastest run, {:.1} us)",
        med as f64 / calls as f64,
        min as f64 / calls as f64,
        min as f64 / 1e3
    );
    println!("records live: {}", rt.live_records());
    if profile {
        return;
    }
    let mut plain = Runtime::<Stub>::new(Config {
        record: false,
        ..Config::default()
    });
    let mut ps = Vec::new();
    for _ in 0..20 {
        let src = source(&lines);
        let t = Instant::now();
        let _ = plain.build(&Stub, Func::Main, &[src]);
        ps.push(t.elapsed().as_nanos());
    }
    ps.sort_unstable();
    println!(
        "plain build (recording off): median {:.1} us",
        ps[ps.len() / 2] as f64 / 1e3
    );
    reads_bench();
}

fn reads_bench() {
    for (calls, reads, slots) in [(100, 10_000, 64), (1000, 1000, 64), (1000, 1000, 1000)] {
        println!(
            "note_read, {calls} calls x {reads} reads of {slots} slots: dense {:.1} ns, hashed {:.1} ns",
            note_reads::<Dense>(calls, reads, slots),
            note_reads::<Sparse>(calls, reads, slots)
        );
    }
}
