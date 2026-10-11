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
    let mut o = opts(false, false);
    o.main = "trip.tex".into();
    let mut b = Build::new(o);
    let out = b.update(vec![Change::File { path: "trip.tex".into(), text: tex }]);
    // trip.tex is all error paths: it must diagnose, not panic, and finish.
    assert!(!out.diags.is_empty());
    assert!(out.dvi.is_some(), "trip ships pages");
}

#[test]
#[ignore = "e-TRIP needs etrip.tfm and a format; slow"]
fn etrip_runs_to_the_end() {
    let tex = bench("etrip", "etrip.tex");
    let mut o = opts(true, false);
    o.main = "etrip.tex".into();
    let mut b = Build::new(o);
    let out = b.update(vec![Change::File { path: "etrip.tex".into(), text: tex }]);
    assert!(!out.diags.is_empty());
}
