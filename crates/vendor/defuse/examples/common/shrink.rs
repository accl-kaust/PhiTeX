//! Edits on a program, their replay against the interpreter, and the
//! minimiser shared by the fuzz, the typing benchmark's failure dump and
//! `examples/shrink.rs`: given a program and an edit history that ends in
//! a failure (incremental != interpreter, an evaluation error, or a broken
//! invariant), drop the edits and lines that are not needed for it.
//!
//! Edits file format, one per line: `set I|TEXT`, `ins I|TEXT`, `rm I`.

use std::cell::Cell;

use super::{Program, Threads, interp::interpret};
use defuse::{Engine, Greedy, RandomScheduler, Sequential, Speculation};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ed {
    Rm(usize),
    Set(usize, String),
    Ins(usize, String),
}

pub fn idx(e: &Ed) -> usize {
    match e {
        Ed::Rm(i) | Ed::Set(i, _) | Ed::Ins(i, _) => *i,
    }
}

pub fn with(e: &Ed, i: usize) -> Ed {
    match e {
        Ed::Rm(_) => Ed::Rm(i),
        Ed::Set(_, l) => Ed::Set(i, l.clone()),
        Ed::Ins(_, l) => Ed::Ins(i, l.clone()),
    }
}

pub fn apply(eng: &mut Engine, p: &mut Program, e: &Ed) {
    match e {
        Ed::Rm(i) => p.remove_line(eng, *i),
        Ed::Set(i, l) => p.set_line(eng, *i, l.clone()),
        Ed::Ins(i, l) => p.insert_line(eng, *i, l.clone()),
    }
}

pub fn edits_text(eds: &[Ed]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for e in eds {
        let _ = match e {
            Ed::Rm(i) => writeln!(s, "rm {i}"),
            Ed::Set(i, l) => writeln!(s, "set {i}|{l}"),
            Ed::Ins(i, l) => writeln!(s, "ins {i}|{l}"),
        };
    }
    s
}

pub fn parse_edits(text: &str) -> Result<Vec<Ed>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let (op, rest) = line
            .split_once(' ')
            .ok_or_else(|| format!("line {}: expected `set I|TEXT`, `ins I|TEXT` or `rm I`", n + 1))?;
        let (i, text) = match rest.split_once('|') {
            Some((i, t)) => (i, Some(t)),
            None => (rest.trim(), None),
        };
        let i: usize = i
            .trim()
            .parse()
            .map_err(|_| format!("line {}: bad index {i:?}", n + 1))?;
        out.push(match (op, text) {
            ("set", Some(t)) => Ed::Set(i, t.to_string()),
            ("ins", Some(t)) => Ed::Ins(i, t.to_string()),
            ("rm", None) => Ed::Rm(i),
            _ => return Err(format!("line {}: bad edit {line:?}", n + 1)),
        });
    }
    Ok(out)
}

/// How a check evaluates.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sched {
    Seq,
    /// Speculative greedy on this many threads.
    Spec(usize),
    /// Random order, a new seed per evaluation (every other one
    /// speculative): the seeds count on from this base at the start of a
    /// replay, so a replay runs the schedules the original run did.
    Random(u64),
    /// Greedy without speculation on this many threads (wide batches
    /// apply in parallel slices).
    Par(usize),
}

impl Sched {
    pub fn parse(s: &str) -> Option<Sched> {
        match s {
            "seq" => Some(Sched::Seq),
            "spec" => Some(Sched::Spec(3)),
            "random" => Some(Sched::Random(0)),
            "par" => Some(Sched::Par(4)),
            s if s.starts_with("random:") => s[7..].parse().ok().map(Sched::Random),
            s => s.strip_prefix("spec:").and_then(|t| t.parse().ok()).map(Sched::Spec),
        }
    }

    pub const ALL: [Sched; 3] = [Sched::Seq, Sched::Spec(3), Sched::Random(0)];
}

/// The seed the next random-order evaluation takes.
pub fn counter() -> u64 {
    COUNTER.with(Cell::get)
}

pub fn set_counter(v: u64) {
    COUNTER.with(|c| c.set(v));
}

thread_local! { static COUNTER: Cell<u64> = const { Cell::new(0) }; }

/// Evaluates; false on an evaluation error.
pub fn evaluate(eng: &mut Engine, sched: Sched) -> bool {
    let m = COUNTER.with(|c| {
        let v = c.get();
        c.set(v.wrapping_add(1));
        v
    });
    match sched {
        Sched::Seq => eng.evaluate_sequential().is_ok(),
        Sched::Par(t) => eng.evaluate(&mut Greedy::default(), &mut Threads(t)).is_ok(),
        Sched::Spec(t) => eng
            .evaluate(&mut Greedy::speculative(Speculation::All, 512), &mut Threads(t))
            .is_ok(),
        Sched::Random(_) => eng
            .evaluate(&mut RandomScheduler::new(m, m.is_multiple_of(2)), &mut Sequential)
            .is_ok(),
    }
}

/// Evaluates and checks the result against the interpreter and the
/// engine's invariants.
pub fn ok(eng: &mut Engine, p: &Program, sched: Sched) -> bool {
    evaluate(eng, sched) && interpret(&p.lines).is_ok_and(|r| r == p.result(eng)) && eng.check_invariants().is_empty()
}

/// Whether `start` is fine cold, every edit but the last keeps it fine, and
/// the last one breaks it.
pub fn replay_bad(start: &[String], eds: &[Ed], sched: Sched) -> bool {
    if let Sched::Random(base) = sched {
        set_counter(base);
    }
    let (mut eng, mut p) = Program::load(start.to_vec());
    if !ok(&mut eng, &p, sched) {
        return false;
    }
    for (n, e) in eds.iter().enumerate() {
        let len = p.lines.len();
        let i = idx(e);
        if i > len || (i == len && !matches!(e, Ed::Ins(..))) {
            return false;
        }
        apply(&mut eng, &mut p, e);
        if !ok(&mut eng, &p, sched) {
            return n + 1 == eds.len();
        }
    }
    false
}

/// `start` without line `q`, and `eds` translated by line identity (edits
/// of that line dropped; an insert keeps its place among the survivors).
pub fn without_line(start: &[String], eds: &[Ed], q: usize) -> Option<(Vec<String>, Vec<Ed>)> {
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

/// `start` without the lines `lo..hi` (edits translated).
pub fn without_lines(start: &[String], eds: &[Ed], lo: usize, hi: usize) -> Option<(Vec<String>, Vec<Ed>)> {
    let mut cur = (start.to_vec(), eds.to_vec());
    for q in (lo..hi).rev() {
        cur = without_line(&cur.0, &cur.1, q)?;
    }
    Some(cur)
}

/// Drops every edit the failure does not need (the last one stays).
fn drop_edits(start: &[String], eds: &mut Vec<Ed>, sched: Sched) {
    let mut q = 0;
    while q + 1 < eds.len() {
        let mut t = eds.clone();
        t.remove(q);
        if replay_bad(start, &t, sched) {
            *eds = t;
        } else {
            q += 1;
        }
    }
}

/// Minimises a failing case: the fewest edits, then the fewest lines
/// (chunks halving down to single lines, delta debugging), then edits
/// again. `progress` is told the size after each reduction.
pub fn minimize(
    start: Vec<String>,
    eds: Vec<Ed>,
    sched: Sched,
    progress: &dyn Fn(usize, usize),
) -> (Vec<String>, Vec<Ed>) {
    assert!(replay_bad(&start, &eds, sched), "the case must fail before shrinking");
    let (mut start, mut eds) = (start, eds);
    drop_edits(&start, &mut eds, sched);
    let mut chunk = (start.len() / 2).max(1);
    loop {
        let mut lo = 0;
        while lo < start.len() {
            let hi = (lo + chunk).min(start.len());
            if let Some((s2, e2)) = without_lines(&start, &eds, lo, hi)
                && replay_bad(&s2, &e2, sched)
            {
                start = s2;
                eds = e2;
                progress(start.len(), eds.len());
            } else {
                lo = hi;
            }
        }
        if chunk == 1 {
            break;
        }
        chunk /= 2;
    }
    drop_edits(&start, &mut eds, sched);
    (start, eds)
}

/// Prints the case: the start, the edits, and the program after them with
/// the incremental rendering against a cold one where they differ.
pub fn report(start: &[String], eds: &[Ed], sched: Sched) {
    println!("START ({} lines)", start.len());
    for (k, l) in start.iter().enumerate() {
        println!("{k:>4} {l}");
    }
    println!("EDITS");
    print!("{}", edits_text(eds));
    println!("SCHED {sched:?}");
    if let Sched::Random(base) = sched {
        set_counter(base);
    }
    let (mut eng, mut p) = Program::load(start.to_vec());
    evaluate(&mut eng, sched);
    for e in eds {
        apply(&mut eng, &mut p, e);
        let fine = evaluate(&mut eng, sched);
        if !fine {
            println!("evaluation error after {e:?}");
        }
    }
    for b in eng.check_invariants().iter().take(5) {
        println!("INV {b}");
    }
    let want = interpret(&p.lines);
    let got = p.result(&eng);
    match &want {
        Ok(w) => println!(
            "diff at {:?}",
            (0..w.len().min(got.len()))
                .filter(|&i| w[i] != got[i])
                .collect::<Vec<_>>()
        ),
        Err(e) => println!("interpreter error: {e}"),
    }
    let (mut e3, p3) = Program::load(p.lines.clone());
    let cold_ok = e3.evaluate_sequential().is_ok();
    let cold = if cold_ok {
        p3.render(&e3)
    } else {
        vec![String::new(); p.lines.len()]
    };
    println!("AFTER (incremental | cold)");
    for ((l, r), c) in p.lines.iter().zip(p.render(&eng)).zip(cold) {
        println!(
            "{l:<44} {r:<20} {}",
            if r == c { String::new() } else { format!("cold: {c}") }
        );
    }
}
