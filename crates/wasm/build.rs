//! The WebAssembly program's memory and stack (wasm32 only): memory
//! up to 4 GiB, shared with the host's workers (their imports name
//! these limits, `scripts/wasm-run.mjs`), and the main thread's stack
//! as large as a native process's (8 MiB), where the build runs.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        for a in [
            "--initial-memory=67108864",
            "--max-memory=4294967296",
            "-zstack-size=8388608",
        ] {
            println!("cargo:rustc-link-arg-bins={a}");
        }
    }
}
