//! `tracing` events for the `observe` build: spans per evaluation and step,
//! events per select, park, wake, commit, round and failure, each with the
//! node's id, operation, unit, depth and state.
//!
//! With `DEFUSE_TRACE=path` (or `stderr`) the engine installs a JSON
//! subscriber at its first evaluation, filtered by `DEFUSE_TRACE_FILTER`
//! (`RUST_LOG` syntax; default `defuse=debug`). Without it the events go
//! to whatever subscriber the client installed, if any.

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use crate::core::{Shared, Slot};

/// Installs the JSON subscriber `DEFUSE_TRACE` asks for (once).
pub(crate) fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(dest) = std::env::var_os("DEFUSE_TRACE") else {
            return;
        };
        let filter = tracing_subscriber::EnvFilter::try_from_env("DEFUSE_TRACE_FILTER")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("defuse=debug"));
        let b = tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_thread_ids(true);
        let _ = if dest == "stderr" {
            b.with_writer(std::io::stderr).try_init()
        } else {
            match std::fs::File::create(&dest) {
                Ok(f) => b.with_writer(Mutex::new(f)).try_init(),
                Err(_) => return,
            }
        };
    });
}

/// One event about node `s`.
pub(crate) fn node(sh: &Shared, s: Slot, what: &'static str, why: &str) {
    if !tracing::enabled!(target: "defuse", tracing::Level::DEBUG) {
        return;
    }
    let r = sh.rec(s);
    let op = r.d.try_lock().map_or("(busy)", |d| sh.op_name(d.op));
    tracing::debug!(
        target: "defuse",
        event = what,
        node = r.id.load(Ordering::Acquire),
        op,
        unit = r.unit.load(Ordering::Acquire),
        depth = r.depth.load(Ordering::Acquire),
        state = r.st(),
        why,
    );
}
