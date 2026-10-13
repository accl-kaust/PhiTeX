use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PATH");
    let suffix = Command::new("pdftex")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            let line = s.lines().next()?;
            let (_, suffix) = line.split_once("1.40.29")?;
            Some(suffix.to_owned())
        })
        .unwrap_or_else(|| " (TeX Live 2026)".to_owned());
    println!("cargo:rustc-env=PHITEX_ORACLE_VERSION_SUFFIX={suffix}");
    println!("cargo:rustc-env=PHITEX_BUILD_ID={:016x}", build_id());
}

/// The sources' hash (every crate's Rust files and the lock file): a
/// cached format is this binary's own, never another build's.
fn build_id() -> u64 {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    files.push(root.join("Cargo.lock"));
    files.sort();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        for b in std::fs::read(f).unwrap_or_default() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}
