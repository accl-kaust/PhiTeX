//! Random edits against the interpreter, with the engine's internal
//! invariants checked after every evaluation. On a failure it shrinks the
//! case (one edit from a cold load, or a replayed edit history).
//!
//! `fuzz POOL SEEDS [FIRST]`: statements are drawn from the lines of POOL
//! (see `examples/fuzz_pool.txt`). `SCHED=random|spec` picks the scheduler
//! (default: sequential greedy).
mod common;
use common::{Program, interp::interpret};
#[derive(Clone, Debug)]
enum Ed {
    Rm(usize),
    Set(usize, String),
    Ins(usize, String),
}
fn apply(eng: &mut defuse::Engine, p: &mut Program, e: &Ed) {
    match e {
        Ed::Rm(i) => p.remove_line(eng, *i),
        Ed::Set(i, l) => p.set_line(eng, *i, l.clone()),
        Ed::Ins(i, l) => p.insert_line(eng, *i, l.clone()),
    }
}
thread_local! { static MODE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
fn ok(eng: &mut defuse::Engine, p: &Program) -> bool {
    eval_mode(eng) && interpret(&p.lines).is_ok_and(|r| r == p.result(eng)) && eng.check_invariants().is_empty()
}
fn eval_mode(eng: &mut defuse::Engine) -> bool {
    let m = MODE.with(|m| {
        let v = m.get();
        m.set(v.wrapping_add(1));
        v
    });
    let r = match std::env::var("SCHED").as_deref() {
        Ok("random") => eng
            .evaluate(
                &mut defuse::RandomScheduler::new(m, m.is_multiple_of(2)),
                &mut defuse::Sequential,
            )
            .map(|_| ()),
        Ok("spec") => eng
            .evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::All, 512),
                &mut common::Threads(3),
            )
            .map(|_| ()),
        _ => eng.evaluate_sequential().map(|_| ()),
    };
    r.is_ok()
}
fn bad(pre: &[String], e: &Ed) -> bool {
    let (mut eng, mut p) = Program::load(pre.to_vec());
    if !ok(&mut eng, &p) {
        return false;
    }
    apply(&mut eng, &mut p, e);
    !ok(&mut eng, &p)
}
fn idx(e: &Ed) -> usize {
    match e {
        Ed::Rm(i) | Ed::Set(i, _) | Ed::Ins(i, _) => *i,
    }
}
fn with(e: &Ed, i: usize) -> Ed {
    match e {
        Ed::Rm(_) => Ed::Rm(i),
        Ed::Set(_, l) => Ed::Set(i, l.clone()),
        Ed::Ins(_, l) => Ed::Ins(i, l.clone()),
    }
}
fn minimize(mut pre: Vec<String>, mut e: Ed) {
    let mut q = 0;
    while q < pre.len() {
        let i = idx(&e);
        if q == i && !matches!(e, Ed::Ins(..)) {
            q += 1;
            continue;
        }
        let mut t = pre.clone();
        t.remove(q);
        let ne = with(&e, if q < i { i - 1 } else { i });
        if bad(&t, &ne) {
            pre = t;
            e = ne;
        } else {
            q += 1;
        }
    }
    println!("MIN {pre:?}\n  {e:?}");
    let (mut eng, mut p) = Program::load(pre.clone());
    eval_mode(&mut eng);
    apply(&mut eng, &mut p, &e);
    println!("eval ok {}", eval_mode(&mut eng));
    for b in eng.check_invariants().iter().take(5) {
        println!("INV {b}");
    }
    let want = interpret(&p.lines).unwrap();
    let got = p.result(&eng);
    println!(
        "diff at {:?}",
        (0..want.len()).filter(|&i| want[i] != got[i]).collect::<Vec<_>>()
    );
    let (mut e3, p3) = Program::load(p.lines.clone());
    e3.evaluate_sequential().unwrap();
    let cold = p3.render(&e3);
    for ((l, r), c) in p.lines.iter().zip(p.render(&eng)).zip(cold) {
        println!(
            "{l:40} {r:20} {}",
            if r == c { String::new() } else { format!("cold: {c}") }
        );
    }
}
fn replay_bad(start: &[String], eds: &[Ed]) -> bool {
    let (mut eng, mut p) = Program::load(start.to_vec());
    if !ok(&mut eng, &p) {
        return false;
    }
    for (n, e) in eds.iter().enumerate() {
        let len = p.lines.len();
        let i = idx(e);
        if i > len || (i == len && !matches!(e, Ed::Ins(..))) {
            return false;
        }
        apply(&mut eng, &mut p, e);
        let good = ok(&mut eng, &p);
        if !good {
            return n + 1 == eds.len();
        }
    }
    false
}
/// `start` without line `q`, and `eds` translated (edits on that line's
/// identity dropped).
fn without_line(start: &[String], eds: &[Ed], q: usize) -> Option<(Vec<String>, Vec<Ed>)> {
    let mut ids: Vec<usize> = (0..start.len()).collect();
    let mut next = start.len();
    let mut red: Vec<usize> = ids.iter().copied().filter(|&i| i != q).collect();
    let mut out = Vec::new();
    for e in eds {
        let i = idx(e);
        match e {
            Ed::Ins(_, l) => {
                let before = ids.get(i).copied();
                ids.insert(i, next);
                let ri = before.map_or(red.len(), |b| red.iter().position(|&x| x == b).unwrap_or(red.len()));
                red.insert(ri, next);
                next += 1;
                out.push(Ed::Ins(ri, l.clone()));
            }
            Ed::Rm(_) | Ed::Set(..) => {
                let id = *ids.get(i)?;
                if matches!(e, Ed::Rm(_)) {
                    ids.remove(i);
                }
                if let Some(ri) = red.iter().position(|&x| x == id) {
                    if matches!(e, Ed::Rm(_)) {
                        red.remove(ri);
                    }
                    out.push(with(e, ri));
                }
            }
        }
    }
    let mut t = start.to_vec();
    t.remove(q);
    Some((t, out))
}
fn shrink_history(hist: &[(Vec<String>, Ed)]) {
    let k = hist.len();
    let mut j = (0..k)
        .rev()
        .find(|&j| {
            let eds: Vec<Ed> = hist[j..].iter().map(|h| h.1.clone()).collect();
            replay_bad(&hist[j].0, &eds)
        })
        .expect("reproduces");
    let start = hist[j].0.clone();
    let mut eds: Vec<Ed> = hist[j..].iter().map(|h| h.1.clone()).collect();
    let mut q = 0;
    while q + 1 < eds.len() {
        let mut t = eds.clone();
        t.remove(q);
        if replay_bad(&start, &t) {
            eds = t;
        } else {
            q += 1;
        }
    }
    // Remove start lines, translating the edits by line identity.
    let mut start = start;
    let mut q = 0;
    while q < start.len() {
        if let Some(t) = without_line(&start, &eds, q)
            && replay_bad(&t.0, &t.1)
        {
            start = t.0;
            eds = t.1;
            continue;
        }
        q += 1;
    }
    let mut q = 0;
    while q + 1 < eds.len() {
        let mut t = eds.clone();
        t.remove(q);
        if replay_bad(&start, &t) {
            eds = t;
        } else {
            q += 1;
        }
    }
    j = 0;
    println!("HIST start ({} lines) {start:?}", start.len());
    for e in &eds {
        println!("  {e:?}");
    }
    let _ = j;
}
fn main() {
    let pool: Vec<String> = std::env::args()
        .nth(1)
        .map(|f| std::fs::read_to_string(f).unwrap().lines().map(String::from).collect())
        .unwrap();
    let seeds: u64 = std::env::args().nth(2).map_or(100, |s| s.parse().unwrap());
    let first: u64 = std::env::args().nth(3).map_or(1, |s| s.parse().unwrap());
    for seed in first..=seeds {
        let mut s: u64 = 0x9e37_79b9_7f4a_7c15 ^ seed;
        let mut next = |m: usize| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            usize::try_from(s % m as u64).unwrap()
        };
        let src: Vec<String> = (0..30).map(|_| pool[next(pool.len())].clone()).collect();
        let (mut eng, mut prog) = Program::load(src.clone());
        if !ok(&mut eng, &prog) {
            println!("COLD seed {seed}\n{src:?}");
            return;
        }
        let mut hist: Vec<(Vec<String>, Ed)> = Vec::new();
        for _ in 0..300 {
            let pre = prog.lines.clone();
            let i = next(prog.lines.len() + 1);
            let e = match next(3) {
                0 if i < prog.lines.len() => Ed::Rm(i),
                1 if i < prog.lines.len() => Ed::Set(i, pool[next(pool.len())].clone()),
                _ => Ed::Ins(i, pool[next(pool.len())].clone()),
            };
            apply(&mut eng, &mut prog, &e);
            if ok(&mut eng, &prog) {
                hist.push((pre, e));
                continue;
            }
            {
                let (mut e2, mut p2) = Program::load(pre.clone());
                let cold_ok = ok(&mut e2, &p2);
                apply(&mut e2, &mut p2, &e);
                let one = ok(&mut e2, &p2);
                let (mut e3, p3) = Program::load(prog.lines.clone());
                let cold_post = ok(&mut e3, &p3);
                for b in eng.check_invariants().iter().take(5) {
                    println!("INV {b}");
                }
                println!(
                    "FAIL seed {seed}: pre-cold-ok {cold_ok}, one-edit-ok {one}, post-cold-ok {cold_post}\n{pre:?}\n{e:?}"
                );
                if cold_ok && !one {
                    minimize(pre, e);
                } else {
                    hist.push((pre, e));
                    shrink_history(&hist);
                }
                return;
            }
        }
    }
    println!("all ok");
}
