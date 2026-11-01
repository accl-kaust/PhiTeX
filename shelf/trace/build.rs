//! The engine's commit (the `phitex` submodule's HEAD), for `--version`:
//! what traced a column, recorded in Shelf's release.json.
fn main() {
    let commit = std::process::Command::new("git")
        .args(["-C", "phitex", "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |s| s.trim().to_owned());
    println!("cargo:rustc-env=PHITEX_COMMIT={commit}");
    println!("cargo:rerun-if-changed=phitex/.git");
    println!("cargo:rerun-if-changed=src/main.rs");
}
