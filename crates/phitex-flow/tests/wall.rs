#![allow(clippy::pedantic)]

//! The wall (AGENTS_BRIEF.md, "How we build it"): phitex-flow depends on
//! `phi` and on the crates listed here, nothing else. Knuth's algorithms
//! are copied out as pure functions, never imported.

/// The dependencies allowed (every section: normal, dev, build, target).
const ALLOWED: &[&str] = &[
    "phi",
    // (XeTeX, user-approved pure libraries, AGENTS_BRIEF §"How we build it":
    // OpenType faces and shaping; XDV to PDF)
    "partex-otf",
    "partex-xdvipdfmx",
    // (pure codecs: the PDF reader XeTeX's \XeTeXpdffile measures with)
    "phitex-codecs",
    // (kpathsea, user-approved pure library: texmf.cnf, ls-R and the path
    // search; the file finder a build is given, `kpse::Kpathsea`)
    "partex-kpse",
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
        assert!(!name.starts_with("partex") || ALLOWED.contains(&name.as_str()), "no partex crate: {l}");
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

/// The library runs no process (the finder, like every file, is a given
/// of the build; wasm has no processes): no source under `src/` but the
/// binaries' names `std::process` or `Command`, before its `#[cfg(test)]`
/// module (a unit test may run an oracle).
#[test]
fn the_library_runs_no_process() {
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).expect("src").flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "bin") {
                    walk(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
    assert!(files.len() > 10, "the sources are listed: {files:?}");
    let mut bad = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).expect("a source");
        let lib = text.split("#[cfg(test)]").next().unwrap_or("");
        for (i, l) in lib.lines().enumerate() {
            let code = l.split("//").next().unwrap_or("");
            if code.contains("std::process") || code.contains("Command::new") {
                bad.push(format!("{}:{}: {}", f.display(), i + 1, l.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "phitex-flow's library runs processes:\n{}", bad.join("\n"));
}
