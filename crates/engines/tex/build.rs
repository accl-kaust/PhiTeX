//! The crate's own identity: a hash of its sources (and the source
//! pass's, whose harvest a format keeps), part of the exported
//! prelude's identity (`api::engine_identity`), so a format written by
//! one build of the crate is never read by a build whose encodings may
//! differ.

use std::path::Path;

fn main() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&src, &mut files);
    // (and the source pass's: the format keeps its harvest of the
    // format's definitions, `api::prelude_defs`)
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../core/source/src"),
        &mut files,
    );
    files.sort();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let bytes = std::fs::read(f).unwrap_or_default();
        for b in f.to_string_lossy().bytes().chain(bytes) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=PHITEX_TEX_SRC_HASH={h:016x}");
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
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
