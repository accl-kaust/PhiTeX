//! The WebAssembly program's memory and stack (wasm32 only): memory
//! 1 GiB at first, up to 4 GiB, shared with the host's workers (their
//! imports name these limits, `scripts/wasm-run.mjs`), and the main
//! thread's stack as large as a native process's (8 MiB), where the
//! build runs. The first gigabyte is reserved, not touched; it is that
//! large so a build rarely grows the memory: V8 checks a bulk memory
//! operation (memory.copy, memory.fill) against a size another thread's
//! grow updates only at its next stack check, so a thread copying into
//! pages just grown traps out of bounds (Phi Frontends, Node 26: 6 of 300
//! builds with 64 MiB at first, 0 of 300 with 1 GiB).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        for a in [
            "--initial-memory=1073741824",
            "--max-memory=4294967296",
            "-zstack-size=8388608",
        ] {
            println!("cargo:rustc-link-arg-bins={a}");
        }
    }
}
