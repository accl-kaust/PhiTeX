//! TRIP / e-TRIP (fixtures in bench/trip, bench/etrip).
mod common;
use common::*;

fn bench(dir: &str, f: &str) -> String {
    read(&root().join("bench").join(dir).join(f))
}

#[test]
#[ignore = "TRIP needs the format dump of pass 1 (tripin) and trip.tfm via a local kpse path; slow"]
fn trip_runs_to_the_end() {
    let tex = bench("trip", "trip.tex");
    let o = Options { main_name: "trip.tex".into(), job_name: "trip".into(), ..opts(false, false) };
    let (_, out) = build_with(o, &tex, &[]);
    // trip.tex is all error paths: it must diagnose, not panic, and finish.
    assert!(!out.diags.is_empty());
    assert!(dvi(&out).is_some(), "trip ships pages");
}

#[test]
#[ignore = "e-TRIP needs etrip.tfm and a format; slow"]
fn etrip_runs_to_the_end() {
    let tex = bench("etrip", "etrip.tex");
    let o = Options { main_name: "etrip.tex".into(), job_name: "etrip".into(), ..opts(true, false) };
    let (_, out) = build_with(o, &tex, &[]);
    assert!(!out.diags.is_empty());
}
