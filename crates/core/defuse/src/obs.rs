//! Probes: counters and spans, compiled in with `profile`, no-ops without.

use crate::lock::Mx;

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
    /// The lock waits by the engine's file that took the lock, ns.
    pub lock_sites: Vec<(&'static str, u64)>,
}

struct Probe {
    name: &'static str,
    count: u64,
    ns: u64,
    span: bool,
}

static PROBES: Mx<Vec<Probe>> = Mx::new(Vec::new());

fn with_probe(name: &'static str, span: bool, f: impl FnOnce(&mut Probe)) {
    if !enabled() {
        return;
    }
    let mut ps = PROBES.lock();
    match ps.iter_mut().find(|p| p.name == name && p.span == span) {
        Some(p) => f(p),
        None => {
            let mut p = Probe {
                name,
                count: 0,
                ns: 0,
                span,
            };
            f(&mut p);
            ps.push(p);
        }
    }
}

pub fn snapshot() -> Snapshot {
    if !enabled() {
        return Snapshot::default();
    }
    let ps = PROBES.lock();
    Snapshot {
        counters: ps.iter().filter(|p| !p.span).map(|p| (p.name, p.count)).collect(),
        spans: ps.iter().filter(|p| p.span).map(|p| (p.name, p.ns, p.count)).collect(),
        lock_wait_ns: ps.iter().filter(|p| p.name == "lock_wait").map(|p| p.ns).sum(),
        lock_sites: crate::lock::SITE_WAITS
            .iter()
            .enumerate()
            .map(|(i, w)| {
                (
                    crate::lock::SITE_FILES.get(i).copied().unwrap_or("other"),
                    w.load(std::sync::atomic::Ordering::Relaxed),
                )
            })
            .collect(),
    }
}

/// Prints the snapshot to stderr when `PHITEX_DEFUSE_PROF` is set.
pub fn flush() {
    if !enabled() || std::env::var_os("PHITEX_DEFUSE_PROF").is_none() {
        return;
    }
    let s = snapshot();
    for (name, n) in &s.counters {
        eprintln!("defuse-prof: {name} {n}");
    }
    for (name, ns, n) in &s.spans {
        eprintln!("defuse-prof: {name} {:.3} ms over {n}", *ns as f64 / 1e6);
    }
    for (f, w) in &s.lock_sites {
        eprintln!("defuse-prof: lock wait taken in {f} {:.3} ms", *w as f64 / 1e6);
    }
}

pub fn reset() {
    if enabled() {
        PROBES.lock().clear();
        for w in &crate::lock::SITE_WAITS {
            w.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Adds `n` to counter `name`.
pub fn count(name: &'static str, n: u64) {
    with_probe(name, false, |p| p.count += n);
}

/// Adds `ns` to span `name`.
pub fn span(name: &'static str, ns: u64) {
    with_probe(name, true, |p| {
        p.ns += ns;
        p.count += 1;
    });
}

/// (`profile`) The lock waits by the file and line that took the lock,
/// the longest first.
#[must_use]
pub fn lock_lines() -> Vec<(String, u64)> {
    crate::lock::site_lines()
}
