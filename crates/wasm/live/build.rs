//! The reactor's memory, stack and exports (wasm32 only). The memory starts
//! at 2 GiB and may reach 65535 pages (64 KiB short of 4 GiB, where the
//! allocator's heap end would wrap to 0); its size is chosen once at the
//! core's start, before any thread runs: the host asks for 4 GiB, then 3,
//! else keeps 2 (`ph_grow`, then `ph_seal`, src/heap.rs; the host's side
//! common/src/wasm/threads.ts), and it never grows after that: growing a
//! shared memory while threads run trips a V8 bug (another thread's
//! memory.copy or memory.fill into the new pages traps). Committed by the
//! system only as it is touched. Shared with the host's workers, each
//! thread's stack 8 MiB as a native process's, and the `ph_*` functions
//! the host calls (a reactor's binary exports none by itself).
//!
//! The sizes (decided 2026-11-04, option B of
//! `docs/note-browser-memory.md`): Chrome's sandbox gives the extension's
//! page 8 GiB of data in all on a Linux machine with 16 GB of memory or
//! less, and a memory's initial size and every growth count against it
//! in full, so a fixed 4 GiB core left no room for a second tab's. A
//! first tab gets 4 GiB, a second beside it 2. What a build touches is
//! what the process holds: about 1.1 GiB before the text, then the
//! engine's state a page (5 to 8 MiB a page of a dense manual, 1.5 of
//! plain text): 3.1 GiB for the beamer user guide's 241 pages at 4
//! threads, less than the native engine's 3.7 GB. The allocator loses
//! about 10% to fragmentation (`--features memstats` counts it). A
//! document that outgrows its core's size stops, and the host says the
//! size it got and what to close. A 2 GiB memory needs a 64-bit browser
//! for its reservation (V8 reserves the maximum and its guard regions);
//! where a browser refuses it the host says so before the first build
//! (scripts/dev/mock-run.mjs, web/test/chrome-probe.mjs). The
//! single-threaded core (wasm32-wasip1, its own memory, no threads)
//! starts at 2 GiB too and grows as it needs, never sealed: with one
//! thread nothing races a growth.

const EXPORTS: [&str; 20] = [
    "ph_grow",
    "ph_seal",
    "ph_alloc",
    "ph_free",
    "ph_open",
    "ph_close",
    "ph_request",
    "ph_lsp",
    "ph_provide",
    "ph_pack",
    "ph_stored",
    "ph_set_index",
    "ph_shell_done",
    "ph_output_ptr",
    "ph_output_len",
    "ph_main_output",
    "ph_pdf",
    "ph_file",
    "ph_strips",
    "ph_frame",
];

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        for a in [
            "--initial-memory=2147483648",
            "--max-memory=4294901760",
            "-zstack-size=8388608",
        ] {
            println!("cargo:rustc-link-arg-bins={a}");
        }
        for e in EXPORTS {
            println!("cargo:rustc-link-arg-bins=--export={e}");
        }
        if std::env::var_os("CARGO_FEATURE_MEMSTATS").is_some() {
            println!("cargo:rustc-link-arg-bins=--export=ph_mem");
        }
    }
}
