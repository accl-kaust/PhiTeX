//! The cost of an edit that moves no loop-back value, `LoopStart::Previous`
//! against `PerRound`: `n` page readers of loop-back `refs` (each about
//! `work` µs), a counter that climbs to `rounds - 1` (so cold takes
//! `rounds` rounds), and `n` plain paragraphs; the edit re-texts one
//! paragraph (its page, which reads it and `refs`, runs again). Prints
//! runs, memo hits, wall and memo bytes per start.
//!
//! `cargo run --release --example loopcost -- [n] [rounds] [work_us] [split]`
//!
//! With `split` the edits alternately insert a paragraph after the middle
//! one and remove it (a paragraph split and join).

use std::sync::Arc;
use std::time::{Duration, Instant};

use defuse::{Ctx, Engine, Field, Locality, LoopStart, Operation, Stop, ValueId, Values};

struct Ops(u64, std::sync::OnceLock<defuse::OpId>);

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
            "piece" => vec![(v.field("piece"), Locality::Local)],
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
                if std::env::var_os("PLACED").is_some() {
                    // Pieces placed after itself, which ones by the round.
                    let me = self.1.get().copied().unwrap();
                    for j in 0..2 {
                        cx.follow(me, vec![v.str("piece"), v.int(x + j), v.int(t)]);
                    }
                }
                Ok(v.int(x * 1000 + t))
            }
            "piece" => {
                spin(self.0);
                let k = v.as_int(a[1]).unwrap() + v.as_int(a[2]).unwrap();
                cx.define(v.field("piece"), v.int(k));
                Ok(v.int(k))
            }
            "reader" => {
                spin(self.0);
                Ok(cx.use_field(v.field("piece"))?.unwrap_or(ValueId::UNIT))
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
    let a: Vec<u64> = std::env::args().skip(1).filter_map(|x| x.parse().ok()).collect();
    let split = std::env::args().any(|x| x == "split");
    let (n, rounds, work) = (
        a.first().copied().unwrap_or(500),
        a.get(1).copied().unwrap_or(4),
        a.get(2).copied().unwrap_or(50),
    );
    println!("{n} pages + {n} paragraphs, {rounds} rounds cold, {work} us per run");
    for start in [LoopStart::Previous, LoopStart::PerRound] {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops(work, std::sync::OnceLock::new()));
        let op = eng.register(ops.clone());
        let _ = ops.1.set(op);
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
            if std::env::var_os("PLACED").is_some() {
                let r = vec![eng.values().str("reader")];
                eng.push(u, op, r);
            }
        }
        let t = Instant::now();
        let r = eng.evaluate_sequential().expect("cold");
        let cold = t.elapsed();
        let mut walls = Vec::new();
        let mut last = None;
        let mut extra = None;
        for k in 0..6i64 {
            if split {
                match extra.take() {
                    None => {
                        let p = vec![eng.values().str("para"), eng.values().int(-1 - k)];
                        extra = Some(eng.insert_after(paras[n as usize / 2], op, p));
                    }
                    Some(x) => eng.remove(x),
                }
            } else {
                let p = vec![eng.values().str("para"), eng.values().int(-1 - k)];
                eng.replace(paras[n as usize / 2], op, p);
            }
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
        if let Some(q) = &e.quick_refused {
            println!("  quick path refused: {q}");
        }
        // Misses by reason, numbers folded.
        let mut by: std::collections::BTreeMap<String, usize> = Default::default();
        for m in &e.memo_misses {
            let w: String = m
                .why
                .chars()
                .map(|c| if c.is_ascii_digit() { '#' } else { c })
                .collect();
            *by.entry(format!("round {}: {w}", m.round)).or_default() += 1;
        }
        for (w, c) in by {
            println!("  miss {c:>6} {w}");
        }
    }
}
