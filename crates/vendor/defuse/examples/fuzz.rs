mod common;
use common::Program;
use common::shrink::{self, Ed, Sched, apply, replay_bad};

thread_local! { static BASE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

/// The scheduler, with the random one's seeds counting on from the base
/// recorded when the current case was loaded.
fn sched() -> Sched {
    match std::env::var("SCHED").as_deref() {
        Ok("random") => Sched::Random(BASE.with(std::cell::Cell::get)),
        Ok("spec") => Sched::Spec(3),
        Ok("spec1") => Sched::Spec(1),
        Ok("par") => Sched::Par(4),
        Ok("workers") => Sched::Workers(4),
        _ => Sched::Seq,
    }
}
fn ok(eng: &mut defuse::Engine, p: &Program) -> bool {
    shrink::ok(eng, p, sched())
}
/// Shrinks a replayed history: the shortest suffix that reproduces, then
/// the fewest edits and lines.
fn shrink_history(hist: &[(Vec<String>, Ed)]) {
    let k = hist.len();
    // A replay from step `j` takes the seeds the run took from there: its
    // load takes edit `j - 1`'s seed, so edit `j` takes the seed it took.
    let from = |j: usize| match sched() {
        Sched::Random(base) => Sched::Random(base + j as u64),
        s => s,
    };
    let j = (0..k)
        .rev()
        .find(|&j| {
            let eds: Vec<Ed> = hist[j..].iter().map(|h| h.1.clone()).collect();
            replay_bad(&hist[j].0, &eds, from(j))
        })
        .expect("reproduces");
    let start = hist[j].0.clone();
    let eds: Vec<Ed> = hist[j..].iter().map(|h| h.1.clone()).collect();
    let (start, eds) = shrink::minimize(start, eds, from(j), &|_, _| {});
    shrink::report(&start, &eds, from(j));
}
fn main() {
    // Stops keep their state and resume (the toy re-executes over it).
    common::REPLAY_STOPS.store(true, std::sync::atomic::Ordering::Relaxed);
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
        BASE.with(|b| b.set(shrink::counter()));
        let (mut eng, mut prog) = Program::load(src.clone());
        if !ok(&mut eng, &prog) {
            println!("COLD seed {seed}\n{src:?}");
            return;
        }
        let mut hist: Vec<(Vec<String>, Ed)> = Vec::new();
        let all = std::env::var_os("FUZZ_DUMP_ALL").is_some();
        // FUZZ_BATCH=k: evaluate (and check) after every k-th edit only, as
        // a client applying several edits between evaluations.
        let batch: usize = std::env::var("FUZZ_BATCH")
            .ok()
            .and_then(|b| b.parse().ok())
            .unwrap_or(1)
            .max(1);
        for step in 0..300 {
            let pre = prog.lines.clone();
            let i = next(prog.lines.len() + 1);
            let e = match next(3) {
                0 if i < prog.lines.len() => Ed::Rm(i),
                1 if i < prog.lines.len() => Ed::Set(i, pool[next(pool.len())].clone()),
                _ => Ed::Ins(i, pool[next(pool.len())].clone()),
            };
            apply(&mut eng, &mut prog, &e);
            if (step + 1) % batch != 0 {
                hist.push((pre, e));
                continue;
            }
            if ok(&mut eng, &prog) || all {
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
                hist.push((pre, e));
                dump(seed, &src, &hist);
                shrink_history(&hist);
                return;
            }
        }
        if all {
            dump(seed, &src, &hist);
        }
    }
    println!("all ok");
}

/// `FUZZ_DUMP=DIR`: a seed's whole history (its program and every edit),
/// for the repro example; at a failure, or with `FUZZ_DUMP_ALL` (which
/// goes on past failures) for every seed.
fn dump(seed: u64, src: &[String], hist: &[(Vec<String>, Ed)]) {
    let Some(d) = std::env::var_os("FUZZ_DUMP") else { return };
    let d = std::path::Path::new(&d);
    let _ = std::fs::create_dir_all(d);
    let _ = std::fs::write(d.join(format!("{seed}.prog")), src.join("\n") + "\n");
    let eds: Vec<String> = hist
        .iter()
        .map(|(_, e)| match e {
            Ed::Rm(i) => format!("rm {i}"),
            Ed::Set(i, t) => format!("set {i}|{t}"),
            Ed::Ins(i, t) => format!("ins {i}|{t}"),
        })
        .collect();
    let _ = std::fs::write(d.join(format!("{seed}.eds")), eds.join("\n") + "\n");
}
