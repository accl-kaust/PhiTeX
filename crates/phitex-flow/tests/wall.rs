#![allow(clippy::pedantic)]

//! The wall (AGENTS_BRIEF.md, "How we build it"): phitex-flow depends on
//! `phi` and nothing else in the workspace. Knuth's algorithms are copied
//! out as pure functions, never imported.

#[test]
fn depends_on_no_partex_crate() {
    let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let mut in_deps = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_deps = l.contains("dependencies");
            continue;
        }
        if !in_deps || l.is_empty() || l.starts_with('#') {
            continue;
        }
        let name = l.split(['=', '.', ' ']).next().unwrap_or("").trim();
        assert!(
            name == "phi" || !(l.contains("path") || l.contains("workspace")),
            "phitex-flow may depend on phi only in the workspace: {l}"
        );
        assert!(!name.starts_with("partex"), "no partex crate: {l}");
    }
}
