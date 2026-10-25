//! Probes: counters and spans, compiled in with `profile`, no-ops without.

/// Whether probes are compiled in.
#[must_use]
pub fn enabled() -> bool {
    cfg!(feature = "profile")
}

/// A snapshot of the probes.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub counters: Vec<(&'static str, u64)>,
    /// (name, nanoseconds, count).
    pub spans: Vec<(&'static str, u64, u64)>,
    pub lock_wait_ns: u64,
}

pub fn snapshot() -> Snapshot {
    todo!()
}

/// Prints the snapshot to stderr when `PHITEX_DEFUSE_PROF` is set.
pub fn flush() {
    todo!()
}

pub fn reset() {
    todo!()
}

/// Adds `n` to counter `name`.
pub fn count(name: &'static str, n: u64) {
    todo!()
}

/// Adds `ns` to span `name`.
pub fn span(name: &'static str, ns: u64) {
    todo!()
}
