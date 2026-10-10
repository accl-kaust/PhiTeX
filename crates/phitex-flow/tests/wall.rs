#![allow(clippy::pedantic)]

//! The wall (AGENTS_BRIEF.md, "How we build it"): phitex-flow depends on
//! `phi` and on the crates listed here, nothing else. Knuth's algorithms
//! are copied out as pure functions, never imported.

/// The dependencies allowed (every section: normal, dev, build, target).
const ALLOWED: &[&str] = &[
    "phi",
    // (the CLI's global allocator, an optional feature: no TeX in it)
    "tikv-jemallocator",
];

/// Each dependency of the manifest: (its name, its line).
fn dependencies(toml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            let h = l.trim_matches(['[', ']']);
            // (`[dependencies.NAME]`: one dependency as a table)
            if let Some((sect, name)) = h.rsplit_once('.')
                && sect.ends_with("dependencies")
            {
                out.push((name.trim_matches('"').to_string(), l.to_string()));
                in_deps = false;
            } else {
                in_deps = h.ends_with("dependencies");
            }
            continue;
        }
        if !in_deps || l.is_empty() || l.starts_with('#') {
            continue;
        }
        let name = l.split(['=', '.', ' ']).next().unwrap_or("").trim().trim_matches('"');
        out.push((name.to_string(), l.to_string()));
    }
    out
}

#[test]
fn depends_on_the_listed_crates_only() {
    let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let deps = dependencies(&toml);
    assert!(deps.iter().any(|(n, _)| n == "phi"), "phi is a dependency");
    for (name, l) in &deps {
        assert!(!name.starts_with("partex"), "no partex crate: {l}");
        assert!(ALLOWED.contains(&name.as_str()), "phitex-flow may depend on {ALLOWED:?} only: {l}");
    }
}

/// The crates of the machine-mode engine (deleted): never in phitex-flow's
/// dependency closure, directly or through another crate.
const NEVER: &[&str] = &["partex-core", "partex-engine", "partex-incr", "partex-ssa", "partex-trace", "phitex-ir", "partex-cli"];

/// The transitive closure too (`cargo tree`, normal and build
/// dependencies): a pure crate that linked an engine crate would bring
/// it in with the direct check still green.
#[test]
fn no_engine_crate_in_the_closure() {
    let out = std::process::Command::new(env!("CARGO"))
        .args(["tree", "-p", "phitex-flow", "-e", "normal,build", "--prefix", "none", "--offline", "--all-features"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree runs");
    assert!(out.status.success(), "cargo tree: {}", String::from_utf8_lossy(&out.stderr));
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(tree.lines().any(|l| l.starts_with("phi ")), "the closure is listed: {tree}");
    for l in tree.lines() {
        let name = l.split(' ').next().unwrap_or("");
        assert!(!NEVER.contains(&name), "phitex-flow's dependency closure holds {name}: {tree}");
    }
}

#[test]
fn the_wall_forbids_unlisted_crates() {
    let bad = "[dependencies]\nphi = { path = \"../phi\" }\npartex-core = { path = \"../partex-core\" }\n\
               [dev-dependencies.serde]\nversion = \"1\"\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n";
    let names: Vec<String> = dependencies(bad).into_iter().map(|d| d.0).collect();
    assert_eq!(names, ["phi", "partex-core", "serde", "libc"]);
}
