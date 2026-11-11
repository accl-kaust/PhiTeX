//! The engine for the frontends: the live document (phitex-live's
//! [`Document`]: the session `phitex watch` keeps, its viewer's protocol,
//! the editor's buffers) as a WASI reactor (wasm32-wasip1-threads), so VS
//! Code's builtin engine and the Overleaf extension run the same session
//! and protocol as native `phitex watch`. The host (common/src/wasm/)
//! keeps the module's memory shared with one worker per engine thread;
//! its thread 0 calls the `ph_*` exports and never blocks, and everything
//! that takes time runs on threads of the module's own:
//!
//! - `ph_open` starts a session thread, which owns the document: it
//!   steps it (the first step is the cold build; a later one builds what
//!   the editor changed), posts its events to the host (`phitex.event`,
//!   JSON: the viewer's `preparing`, `diagnostics`, `origins`, `settled`,
//!   ..., as they happen, and the build's `build` or `failed` after it),
//!   then waits until something is asked.
//! - A viewer's request (`ph_request`, `session.ts`'s `CoreReq`) is
//!   answered at once on thread 0 by the document's viewer, from its state
//!   (each lock held for a copy, never across a build); what it asks of the
//!   document (`set_file`, `edit`, `rebuild`) is queued and wakes the
//!   session thread.
//! - The editor speaks to the document's language server
//!   (phitex-live's `lsp::Server`, docs/spec-editor-protocol.md, as
//!   `phitex watch --lsp`): a JSON-RPC message in (`ph_lsp`), the server's
//!   out to the host (`phitex.lsp`): the buffers, the cursor, the
//!   commands, the build's progress, problems and status, a source
//!   place shown (`window/showDocument`). What the host runs itself (the
//!   document stopped, the settings changed) is an event to it.
//! - A file the build lacks is resolved by Shelf's index (`ph_set_index`,
//!   phitex-shelf) and its pack asked of the host (`phitex.fetch`), or
//!   asked by name (`phitex.want`), by the thread that needs it, which
//!   waits until thread 0 answers (`ph_pack`, `ph_provide`): [`files`];
//!   each fetch the build's progress.
//! - What the session keeps between sessions (the format's evaluated
//!   state, the records) is the host's store, asked the same way
//!   (`phitex.store_get`, answered by `ph_stored`) and put without a wait
//!   (`phitex.store_put`): [`store`].
//! - A `\write18` the engine allows (latexminted) is posted to the host
//!   (`phitex.shell`), which runs it and answers (`ph_shell_done`):
//!   [`shell`], only when the open says the host has a runner.
//! - The outputs stay here ([`live::Memory`]): the host reads the main one
//!   (`ph_main_output`), the PDF the viewer is shown (`ph_pdf`) or any
//!   file the build wrote (`ph_file`).
//!
//! Requests and answers are bytes in the module's memory: the host gets
//! room with `ph_alloc`, and takes an answer's bytes at `ph_output_ptr`,
//! `ph_output_len`.

// (the host's imports and the `ph_*` exports: raw pointers, the crate's
// unsafe code with heap's and mem's, each block's SAFETY saying why it is sound)
#[allow(
    unsafe_code,
    reason = "the host's side: its imports and the exports' pointers"
)]
#[cfg(target_arch = "wasm32")]
mod abi;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "the wasm core's files; their stamps tested natively"
    )
)]
mod files;
#[allow(
    unsafe_code,
    reason = "the memory's size chosen at the start: the system allocator's own calls"
)]
#[cfg(target_arch = "wasm32")]
mod heap;
#[cfg(target_arch = "wasm32")]
mod live;
#[cfg(all(target_arch = "wasm32", feature = "memstats"))]
#[allow(
    unsafe_code,
    reason = "the system allocator wrapped to count: its own calls, as the caller's"
)]
mod mem;
#[cfg(all(target_arch = "wasm32", feature = "memstats"))]
#[global_allocator]
static GLOBAL: mem::Counted = mem::Counted;
// (natively too under test: its frames' tests run with the workspace's)
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "the hook is the wasm core's; its frames tested natively"
    )
)]
mod shell;
#[cfg(target_arch = "wasm32")]
mod store;
// (the document, the session thread and what the exports call: safe code)
#[cfg(target_arch = "wasm32")]
mod session;

/// Whether the module has threads (wasm32-wasip1-threads: one shared
/// memory, a worker a thread). Without (wasm32-wasip1, Firefox: no shared
/// memory for an extension) the document has no session thread: the host
/// steps it (`ph_step`), and answers every ask inside its call.
#[cfg(any(target_arch = "wasm32", test))]
pub const THREADED: bool = cfg!(target_feature = "atomics");

// (natively an empty program: the crate is the wasm32 core, built by
// scripts/wasm-live.sh; the workspace's native checks build it too)
fn main() {}
