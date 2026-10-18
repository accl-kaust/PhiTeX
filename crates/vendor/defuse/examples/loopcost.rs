//! The cost of an edit that moves no loop-back value, `LoopStart::Previous`
//! against `PerRound`: `n` page readers of loop-back `refs` (each about
//! `work` µs), a counter that climbs to `rounds - 1` (so cold takes
//! `rounds` rounds), and `n` plain paragraphs; the edit re-texts one
//! paragraph (its page, which reads it and `refs`, runs again). Prints
//! runs, memo hits, wall and memo bytes per start.
//!
//! `cargo run --release --example loopcost -- [n] [rounds] [work_us]`

use std::sync::Arc;
use std::time::{Duration, Instant};

use defuse::{Ctx, Engine, Field, Locality, LoopStart, Operation, Stop, ValueId, Values};

struct Ops(u64);

fn spin(us: u64) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_micros(us) {
        std::hint::spin_loop();
    }
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match v.as_str(a[0]).unwrap_or_default().as_str() {
            "para" => vec![(v.field("text"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let refs = v.field("refs");
        match v.as_str(a[0]).unwrap_or_default().as_str() {
            "count" => {
                let k = v.as_int(a[1]).unwrap();
                let x = cx.use_field(refs)?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.publish(refs, v.int((x + 1).min(k)));
                Ok(v.int(x))
            }
            "page" => {
                spin(self.0);
                let x = cx.use_field(refs)?.and_then(|x| v.as_int(x)).unwrap_or(0);
                // Its paragraph's text (the nearest before it).
                let t = cx.use_field(v.field("text"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                Ok(v.int(x * 1000 + t))
            }
            _ => {
                spin(self.0);
                cx.define(v.field("text"), a[1]);
                Ok(a[1])
            }
        }
    }
}

fn main() {
    let a: Vec<u64> = std::env::args().skip(1).map(|x| x.parse().unwrap()).collect();
    let (n, rounds, work) = (
        a.first().copied().unwrap_or(500),
        a.get(1).copied().unwrap_or(4),
        a.get(2).copied().unwrap_or(50),
    );
    println!("{n} pages + {n} paragraphs, {rounds} rounds cold, {work} us per run");
    for start in [LoopStart::Previous, LoopStart::PerRound] {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Ops(work)));
        let u = eng.new_unit();
        let (refs, zero) = (eng.values().field("refs"), eng.values().int(0));
        eng.declare_loopback(refs, zero);
        eng.set_loop_start(start);
        let c = vec![
            eng.values().str("count"),
            eng.values().int(i64::try_from(rounds).unwrap() - 1),
        ];
        eng.push(u, op, c);
        let mut paras = Vec::new();
        for i in 0..n {
            let p = vec![eng.values().str("para"), eng.values().int(i64::try_from(i).unwrap())];
            paras.push(eng.push(u, op, p));
            let g = vec![eng.values().str("page"), eng.values().int(i64::try_from(i).unwrap())];
            eng.push(u, op, g);
        }
        let t = Instant::now();
        let r = eng.evaluate_sequential().expect("cold");
        let cold = t.elapsed();
        let mut walls = Vec::new();
        let mut last = None;
        for k in 0..5i64 {
            let p = vec![eng.values().str("para"), eng.values().int(-1 - k)];
            eng.replace(paras[n as usize / 2], op, p);
            let t = Instant::now();
            let r = eng.evaluate_sequential().expect("edit");
            walls.push(t.elapsed().as_secs_f64() * 1e3);
            last = Some(r);
        }
        let e = last.unwrap();
        let hits: u64 = e.round_times.iter().map(|t| t.memo_hits).sum();
        walls.sort_by(f64::total_cmp);
        println!(
            "{start:?}: cold {:.1} ms ({} rounds, {} runs); edit median {:.3} ms, {} runs, {} memo hits, {} rounds, memo {} KB",
            cold.as_secs_f64() * 1e3,
            r.rounds,
            r.runs,
            walls[2],
            e.runs,
            hits,
            e.rounds,
            e.memo_bytes / 1024
        );
    }
}
