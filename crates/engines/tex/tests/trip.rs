//! Knuth's trip test (tex.web §1379) and e-TeX's etrip, through the
//! engine: the first pass (`\input trip` up to `\dump`) is the prelude,
//! the second pass (`&trip trip`) the document. The oracle is the
//! installed `tex -ini` / `etex -ini` on the same files
//! (`oracle/fixtures/trip`: `trip.dvi`, `etrip.dvi`, their dvitype
//! listings `*.typ`). Only the DVI is compared; the parts of the test
//! that overflow capacities on purpose are not comparable (we do not
//! emulate capacities), so each run is time-capped. Heavy: ignored.

mod common;

use phitex_tex::Options;

fn trip(name: &str, prelude_jobname: &str) {
    let dir = common::fixtures().join("trip");
    let src = common::read(&dir.join(format!("{name}.tex")));
    let want = std::fs::read(dir.join(format!("{name}.dvi"))).expect("the oracle");
    let opts = Options {
        prelude_jobname: prelude_jobname.to_owned(),
        time_limit: Some(std::time::Duration::from_secs(120)),
        ..common::options(
            &format!("./{name}.tex"),
            name,
            &common::dvi_comment(&want),
            &dir,
        )
    };
    let t = std::time::Instant::now();
    let b = common::build(opts, Some(&format!("\\input {name}\n")), &src);
    eprintln!("{name}: {:?}", t.elapsed());
    for d in b.diagnostics().iter().take(40) {
        eprintln!(
            "{:?} {}: {}",
            d.severity,
            d.line,
            d.message.chars().take(300).collect::<String>()
        );
    }
    let (got, log) = b.output();
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let _ = std::fs::write(out.join(format!("{name}.dvi")), &got);
    let _ = std::fs::write(out.join(format!("{name}.log")), &log);
    if let Some(d) = common::first_difference(&got, &want) {
        panic!("{name}.dvi: {d}");
    }
}

#[test]
#[ignore = "heavy: the trip test"]
fn trip_test() {
    trip("trip", "trip");
}

#[test]
#[ignore = "heavy: the etrip test"]
fn etrip_test() {
    trip("etrip", "etrip");
}
