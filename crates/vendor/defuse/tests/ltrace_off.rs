//! Without the `ltrace` feature the light trace is compiled out: this test
//! binary, which links the engine, holds no trace switch name (the trace
//! reads it from the environment) and evaluations still run.

#[cfg(not(feature = "ltrace"))]
#[test]
fn a_default_build_carries_no_trace() {
    let mut eng = defuse::Engine::new();
    eng.evaluate_sequential().expect("an empty evaluation");
    let bin = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    // (Spelled backwards: the test must not hold the name itself.)
    let needle: String = "ECARTL_ESUFED".chars().rev().collect();
    assert!(
        !bin.windows(needle.len()).any(|w| w == needle.as_bytes()),
        "the light trace is linked into a default build"
    );
}
