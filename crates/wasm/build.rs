//! The WebAssembly program's memory and stack (wasm32 only): with threads
//! (wasm32-wasip1-threads), memory at its maximum from the start, 65535 pages (4 GiB less one page), shared
//! with the host's workers (their imports name these limits,
//! `scripts/wasm-run.mjs`), and the main thread's stack as large as a
//! native process's (8 MiB). The memory never grows, so V8's race cannot
//! happen: it checks memory.copy and memory.fill against a size another
//! thread's memory.grow updates only at the copying thread's next stack
//! check, so a thread copying into pages just grown traps out of bounds
//! (Phi Frontends: 6 of 300 builds trapped with memory that grew, 0 of
//! 231 at initial = maximum; Node 26 and Chromium 153 both, a 50-byte
//! module repeats it). Pages are committed as touched: a build's
//! resident size is unchanged (903 MiB against 891). 65536 pages would
//! put the heap's end at 2^32, which wraps to 0 and traps dlmalloc's
//! first allocation.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        // (the memory at its maximum only where threads share it; the
        // single-threaded build, wasm32-wasip1, has no race: its memory
        // grows as it needs. Its main stack a worker's, 16 MiB
        // (phitex_defuse::WORKER_STACK): the evaluation runs on it, not
        // on a worker of its own)
        let atomics = std::env::var("CARGO_CFG_TARGET_FEATURE")
            .is_ok_and(|f| f.split(',').any(|x| x == "atomics"));
        let stack = if atomics { 8 << 20 } else { 16 << 20 };
        println!("cargo:rustc-link-arg-bins=-zstack-size={stack}");
        if atomics {
            for a in ["--initial-memory=4294901760", "--max-memory=4294901760"] {
                println!("cargo:rustc-link-arg-bins={a}");
            }
        }
    }
}
