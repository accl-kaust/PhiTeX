use std::path::{Path, PathBuf};
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

/// FNV-1a, 64 bits, as `tex::Fnv`.
struct Fnv(u64);

impl Fnv {
    fn add(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x100_0000_01b3);
    }
}

/// The files under `dir`, recursively, in a fixed order.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// What the binary is built from, as a hash: the same for the same sources
/// in any checkout (no paths, times or the executable's bytes, which hold
/// the checkout's path): each workspace crate's `Cargo.toml`, `build.rs`,
/// `src/` and `viewer/` (the files it embeds), the workspace's
/// `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` (paths relative to
/// the workspace), the compiler's version and this crate's features. A
/// format is cached under it (`format::ensure`).
fn build_id() -> u64 {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut paths = Vec::new();
    for f in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        paths.push(root.join(f));
    }
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    crates.sort();
    for c in crates {
        paths.push(c.join("Cargo.toml"));
        paths.push(c.join("build.rs"));
        for d in ["src", "viewer"] {
            files(&c.join(d), &mut paths);
            // (a directory is scanned whole; a path that does not exist
            // would run this again at every build)
            if c.join(d).is_dir() {
                println!("cargo:rerun-if-changed={}", c.join(d).display());
            }
        }
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    for p in &paths {
        let Ok(bytes) = std::fs::read(p) else { continue };
        if !p.components().any(|c| c.as_os_str() == "src" || c.as_os_str() == "viewer") {
            println!("cargo:rerun-if-changed={}", p.display());
        }
        let rel = p.strip_prefix(&root).unwrap_or(p);
        h.add(rel.to_string_lossy().as_bytes());
        h.add(&bytes);
    }
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    if let Ok(o) = Command::new(rustc).arg("-V").output() {
        h.add(&o.stdout);
    }
    let mut features: Vec<String> = std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("CARGO_FEATURE_")).collect();
    features.sort();
    h.add(features.join(",").as_bytes());
    h.0
}
