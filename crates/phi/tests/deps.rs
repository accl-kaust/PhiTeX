//! φ depends on no crate of the workspace: no TeX or PDF crate can creep
//! in (DESIGN 7.13).

#[test]
fn no_workspace_dependencies() {
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
        let name = l.split(['=', '.', ' ']).next().unwrap_or("");
        assert!(
            !l.contains("path") && !l.contains("workspace") && !name.starts_with("partex") && !name.starts_with("phitex"),
            "crates/phi must not depend on the workspace: {l}"
        );
    }
}
