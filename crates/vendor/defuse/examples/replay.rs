//! Trace replay: loads a JSONL trace (see `common/trace.rs` for the format),
//! evaluates it cold, then applies an edits file (or generated edits),
//! timing each and checking the incremental result against a cold replay.
//!
//! `cargo run --release --example replay -- TRACE.jsonl [EDITS.jsonl]
//!     [--threads N] [--spec off|predicted|all] [--check] [--check-every K]
//!     [--env FIELD=INT]...`
//! `cargo run --release --example replay -- --from-toy N [OUT.jsonl]`
//!     writes a trace derived from the generated N-line toy program (and
//!     `OUT.edits.jsonl` with 200 generated edits), or replays it directly.
//!
//! Malformed input is rejected with the line and the reason.

mod common;

use std::time::{Duration, Instant};

use common::Threads;
use common::trace::{self, Doc, Edit, EditLine, Trace};
use defuse::{Engine, Field, Greedy, Report, Speculation, ValueId};

/// One edit's wall time, nodes run, and the trace's expectation.
type Sample = (Duration, u64, Option<u64>);

struct Opts {
    threads: usize,
    spec: Speculation,
    check: bool,
    check_every: usize,
    env: Vec<(String, i64)>,
}

fn eval(eng: &mut Engine, o: &Opts) -> Result<Report, String> {
    let r = if o.spec == Speculation::Off && o.threads <= 1 {
        eng.evaluate_sequential()
    } else {
        eng.evaluate(&mut Greedy::speculative(o.spec, 256), &mut Threads(o.threads))
    };
    r.map_err(|e| format!("{e:?}"))
}

fn env_of(eng: &Engine, t: &Trace, o: &Opts) -> Vec<(Field, ValueId)> {
    let v = eng.values();
    let mut env: Vec<(Field, ValueId)> = t
        .meta
        .env
        .iter()
        .map(|(f, x)| {
            let val = match x {
                common::json::Json::Int(i) => v.int(*i),
                common::json::Json::Bool(b) => v.bool(*b),
                common::json::Json::Str(s) => v.str(s),
                _ => ValueId::UNIT,
            };
            (v.field(f), val)
        })
        .collect();
    for (f, i) in &o.env {
        env.retain(|(g, _)| *g != v.field(f));
        env.push((v.field(f), v.int(*i)));
    }
    env
}

fn cold(t: &Trace, o: &Opts) -> Result<(Engine, Doc, Report, Duration, Duration), String> {
    let t0 = Instant::now();
    let mut eng = Engine::new();
    let doc = Doc::load(&mut eng, t);
    eng.set_environment(env_of(&eng, t, o));
    let load = t0.elapsed();
    let t1 = Instant::now();
    let r = eval(&mut eng, o)?;
    Ok((eng, doc, r, load, t1.elapsed()))
}

fn pct(ts: &mut [Duration], p: usize) -> Duration {
    ts.sort();
    ts[(ts.len() - 1) * p / 100]
}

#[allow(clippy::too_many_lines)]
fn main() {
    let mut args = std::env::args().skip(1);
    let mut o = Opts {
        threads: 1,
        spec: Speculation::Off,
        check: false,
        check_every: 0,
        env: Vec::new(),
    };
    let mut paths = Vec::new();
    let mut from_toy: Option<usize> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--threads" => o.threads = args.next().and_then(|s| s.parse().ok()).expect("--threads N"),
            "--spec" => {
                o.spec = match args.next().as_deref() {
                    Some("off") => Speculation::Off,
                    Some("predicted") => Speculation::Predicted,
                    Some("all") => Speculation::All,
                    _ => panic!("--spec off|predicted|all"),
                }
            }
            "--check" => o.check = true,
            "--check-every" => o.check_every = args.next().and_then(|s| s.parse().ok()).expect("--check-every K"),
            "--env" => {
                let kv = args.next().expect("--env FIELD=INT");
                let (f, x) = kv.split_once('=').expect("--env FIELD=INT");
                o.env.push((f.to_string(), x.parse().expect("--env FIELD=INT")));
            }
            "--from-toy" => from_toy = Some(args.next().and_then(|s| s.parse().ok()).expect("--from-toy N")),
            s => paths.push(s.to_string()),
        }
    }
    if o.check && o.check_every == 0 {
        o.check_every = 1;
    }
    let (trace, edits) = if let Some(n) = from_toy {
        let lines = common::generate_with(42, n, true);
        let t = trace::from_toy(&lines, &format!("toy-{n}"));
        let edits = trace::generated_edits(&t, 7, 200);
        if let Some(out) = paths.first() {
            std::fs::write(out, trace::trace_jsonl(&t)).expect("write trace");
            let ep = format!("{}.edits.jsonl", out.trim_end_matches(".jsonl"));
            let mut text = String::new();
            for e in &edits {
                text.push_str(&edit_json(&e.edit));
                text.push('\n');
            }
            std::fs::write(&ep, text).expect("write edits");
            println!("wrote {out} ({} steps) and {ep} ({} edits)", t.steps.len(), edits.len());
            return;
        }
        (t, edits)
    } else {
        let Some(tp) = paths.first() else {
            eprintln!(
                "usage: replay TRACE.jsonl [EDITS.jsonl] [--threads N] [--spec off|predicted|all] [--check] | --from-toy N [OUT.jsonl]"
            );
            std::process::exit(2);
        };
        let text = std::fs::read_to_string(tp).unwrap_or_else(|e| {
            eprintln!("{tp}: {e}");
            std::process::exit(2);
        });
        let t = trace::load_trace(&text).unwrap_or_else(|e| {
            eprintln!("{tp}: {e}");
            std::process::exit(1);
        });
        let edits = match paths.get(1) {
            Some(ep) => {
                let text = std::fs::read_to_string(ep).unwrap_or_else(|e| {
                    eprintln!("{ep}: {e}");
                    std::process::exit(2);
                });
                trace::load_edits(&text).unwrap_or_else(|e| {
                    eprintln!("{ep}: {e}");
                    std::process::exit(1);
                })
            }
            None => trace::generated_edits(&t, 7, 200),
        };
        (t, edits)
    };
    println!(
        "trace {:?}: {} steps, {} sequences, {} env fields; {} edits; threads {} spec {:?}",
        trace.meta.document,
        trace.steps.len(),
        trace.meta.sequences.len(),
        trace.meta.env.len(),
        edits.len(),
        o.threads,
        o.spec
    );
    let (mut eng, mut doc, r, load, t) = cold(&trace, &o).unwrap_or_else(|e| {
        eprintln!("cold evaluation failed: {e}");
        std::process::exit(1);
    });
    println!(
        "load {load:.3?}  cold {t:.3?}  runs {}  stops {}  gated {}  pending waits {}  pending sets {} (defining {})  speculations {} confirmed {} wrong {}  live {}  critical path {}",
        r.runs,
        r.stops,
        r.gated,
        r.pending_waits,
        r.pending_sets,
        r.pending_sets_defining,
        r.speculations,
        r.confirmations,
        r.mispredictions,
        eng.live_nodes(),
        eng.critical_path()
    );
    let causes = trace::causes(&eng, &doc);
    if !causes.is_empty() {
        let items: Vec<String> = causes
            .iter()
            .map(|(c, (n, m))| format!("{c}: {n} pending, {m} as prior"))
            .collect();
        println!("causes: {}", items.join("; "));
    }
    if o.check {
        let (e2, d2, _, _, _) = cold(&trace, &o).expect("second cold");
        assert_eq!(doc.result(&eng), d2.result(&e2), "two cold evaluations differ");
    }
    let mut stats: Vec<(&'static str, Vec<Sample>)> = vec![
        ("change_writes", Vec::new()),
        ("change_reads", Vec::new()),
        ("insert_after", Vec::new()),
        ("delete", Vec::new()),
    ];
    let mut mismatches = 0;
    for (k, EditLine { edit, expect_reached }) in edits.iter().enumerate() {
        let kind = match edit {
            Edit::ChangeWrites(..) => 0,
            Edit::ChangeReads(..) => 1,
            Edit::InsertAfter(..) => 2,
            Edit::Delete(_) => 3,
        };
        let t0 = Instant::now();
        if let Err(e) = doc.apply(&mut eng, edit) {
            eprintln!("edit {}: {e}", k + 1);
            std::process::exit(1);
        }
        let r = eval(&mut eng, &o).unwrap_or_else(|e| {
            eprintln!("edit {}: evaluation failed: {e}", k + 1);
            std::process::exit(1);
        });
        let wall = t0.elapsed();
        if let Some(want) = expect_reached
            && *want != r.nodes_run
        {
            mismatches += 1;
            println!("edit {}: {} nodes ran, the trace expected {want}", k + 1, r.nodes_run);
        }
        stats[kind].1.push((wall, r.nodes_run, *expect_reached));
        if o.check_every > 0 && (k + 1) % o.check_every == 0 {
            let snap = doc.snapshot();
            let (e2, d2, _, _, _) = cold(&snap, &o).unwrap_or_else(|e| {
                eprintln!("edit {}: cold replay failed: {e}", k + 1);
                std::process::exit(1);
            });
            if doc.result(&eng) != d2.result(&e2) {
                eprintln!("edit {}: incremental result differs from a cold replay", k + 1);
                std::process::exit(1);
            }
        }
    }
    println!(
        "{:<14} {:>5} {:>11} {:>11} {:>8} {:>10}",
        "edit", "n", "p50", "p99", "run~", "expected~"
    );
    for (name, s) in &mut stats {
        if s.is_empty() {
            continue;
        }
        let mut ts: Vec<Duration> = s.iter().map(|x| x.0).collect();
        let mut runs: Vec<u64> = s.iter().map(|x| x.1).collect();
        runs.sort_unstable();
        let mut exp: Vec<u64> = s.iter().filter_map(|x| x.2).collect();
        exp.sort_unstable();
        println!(
            "{name:<14} {:>5} {:>11.3?} {:>11.3?} {:>8} {:>10}",
            s.len(),
            pct(&mut ts, 50),
            pct(&mut ts, 99),
            runs[runs.len() / 2],
            exp.get(exp.len() / 2)
                .map_or_else(|| "-".to_string(), ToString::to_string)
        );
    }
    if mismatches > 0 {
        println!("{mismatches} edits ran a different number of nodes than the trace expected");
    }
    if o.check_every > 0 {
        println!("incremental results equal cold replays at every checked edit");
    }
}

fn edit_json(e: &Edit) -> String {
    match e {
        Edit::ChangeWrites(id, ws) => {
            let w = match ws {
                None => "null".to_string(),
                Some(ws) => {
                    let items: Vec<String> = ws
                        .iter()
                        .map(|w| {
                            format!(
                                "{{\"field\": {}, \"locality\": \"{}\"}}",
                                common::json::quote(&w.field),
                                if w.local { "local" } else { "global" }
                            )
                        })
                        .collect();
                    format!("[{}]", items.join(", "))
                }
            };
            format!(
                "{{\"op\": \"change_writes\", \"step\": {}, \"writes\": {w}}}",
                common::json::quote(id)
            )
        }
        Edit::ChangeReads(id, rs) => {
            let items: Vec<String> = rs.iter().map(|r| common::json::quote(r)).collect();
            format!(
                "{{\"op\": \"change_reads\", \"step\": {}, \"reads\": [{}]}}",
                common::json::quote(id),
                items.join(", ")
            )
        }
        Edit::InsertAfter(id, s) => {
            format!(
                "{{\"op\": \"insert_after\", \"step\": {}, \"new\": {}}}",
                common::json::quote(id),
                trace::step_json(s)
            )
        }
        Edit::Delete(id) => format!("{{\"op\": \"delete\", \"step\": {}}}", common::json::quote(id)),
    }
}
