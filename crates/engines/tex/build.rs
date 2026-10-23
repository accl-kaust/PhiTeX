//! The crate's own identity: a hash of its sources and of every crate it
//! depends on by path (the source pass's, whose harvest a format keeps,
//! the typesetter's, the fonts' and the outputs', whose values a format
//! holds), part of the exported prelude's identity
//! (`api::engine_identity`), so a format written by one build of the
//! engine is never read by a build whose encodings may differ, and one
//! built by a binary that differs elsewhere (its command line, its
//! viewer) is read as it is.

use std::path::{Path, PathBuf};

fn main() {
    let mut crates = Vec::new();
    closure(&Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap_or_default(), &mut crates);
    let mut files = Vec::new();
    for c in &crates {
        files.push(c.join("Cargo.toml"));
        collect(&c.join("src"), &mut files);
    }
    files.sort();
    // (the names hashed under `crates/`: the same in every checkout)
    let root = crates[0].join("../..").canonicalize().unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let bytes = std::fs::read(f).unwrap_or_default();
        let name = f.strip_prefix(&root).unwrap_or(f);
        for b in name.to_string_lossy().bytes().chain(bytes) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=PHITEX_TEX_SRC_HASH={h:016x}");
}

/// The crate at `dir` and those it depends on by path (its
/// `[dependencies]` and its targets', not its dev-dependencies), each
/// once.
fn closure(dir: &Path, out: &mut Vec<PathBuf>) {
    if out.iter().any(|d| d == dir) {
        return;
    }
    out.push(dir.to_path_buf());
    let toml = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
    let mut deps = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            deps = line == "[dependencies]"
                || (line.starts_with("[target.") && line.ends_with(".dependencies]"));
            continue;
        }
        if !deps {
            continue;
        }
        let Some(rest) = line.split("path").nth(1) else {
            continue;
        };
        let Some(path) = rest.trim_start().strip_prefix('=').and_then(|r| r.split('"').nth(1))
        else {
            continue;
        };
        if let Ok(d) = dir.join(path).canonicalize() {
            closure(&d, out);
        }
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}
