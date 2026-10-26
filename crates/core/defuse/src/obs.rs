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

/// (`profile`) A commit's time under its segment's lock by part, reported
/// for one that held it past a millisecond.
pub(crate) struct Laps {
    #[cfg(feature = "profile")]
    t: std::time::Instant,
    #[cfg(feature = "profile")]
    parts: Vec<(&'static str, u64)>,
}

impl Laps {
    #[allow(unused_variables)]
    pub(crate) fn new(t: std::time::Instant) -> Laps {
        Laps {
            #[cfg(feature = "profile")]
            t,
            #[cfg(feature = "profile")]
            parts: Vec::new(),
        }
    }

    #[allow(unused_variables, clippy::unused_self)]
    pub(crate) fn lap(&mut self, name: &'static str) {
        #[cfg(feature = "profile")]
        {
            let now = std::time::Instant::now();
            self.parts.push((name, u64::try_from(now.duration_since(self.t).as_nanos()).unwrap_or(u64::MAX)));
            self.t = now;
        }
    }

    #[allow(unused_variables, clippy::unused_self)]
    pub(crate) fn report(&self, node: u64, ns: u64, defs: usize, reads: usize) {
        #[cfg(feature = "profile")]
        if ns > 1_000_000 {
            let parts: Vec<String> = self.parts.iter().map(|(n, t)| format!("{n} {:.3}", *t as f64 / 1e6)).collect();
            eprintln!("commit-laps: node {node} {:.3} ms ({defs} defs, {reads} reads): {}", ns as f64 / 1e6, parts.join(", "));
        }
    }
}

/// (`profile`) A hold of segment locks past a millisecond, reported:
/// what held them and for how long.
#[allow(unused_variables)]
pub(crate) fn long_hold(what: &str, n: usize, t: std::time::Instant) {
    #[cfg(feature = "profile")]
    {
        let ns = t.elapsed().as_nanos();
        if ns > 1_000_000 {
            eprintln!("long-hold: {what} ({n}) {:.3} ms", ns as f64 / 1e6);
        }
    }
}


/// (`PHITEX_LINE_DUMP=<n>`, profile, with `PHITEX_WAKE_LOG`) Every `n`
/// wakes, the line: each live node in position order with its position,
/// id, operation, state, arguments, and its definitions of the fields
/// named in `PHITEX_LINE_FIELDS` (comma-separated).
pub(crate) fn maybe_dump_line(eng: &crate::engine::Engine) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static EVERY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let every = *EVERY.get_or_init(|| {
        std::env::var("PHITEX_LINE_DUMP").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
    });
    if every == 0 {
        return;
    }
    let c = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if !c.is_multiple_of(every) {
        return;
    }
    let fields: Vec<String> = std::env::var("PHITEX_LINE_FIELDS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let v = eng.values();
    let fs: Vec<crate::values::Field> = fields.iter().map(|f| v.field(f)).collect();
    let mut out = format!("line-dump #{c}:\n");
    for sid in eng.order.all() {
        let seg = eng.seg(sid);
        let inner = seg.inner.read();
        for (pos, nid) in &inner.nodes {
            let n = eng.node(*nid);
            let op = eng.ops.get(n.op().0 as usize).map_or("?", |o| o.name());
            let args: Vec<String> = n.args().iter().map(|&a| v.render(a)).collect();
            let mut args = args.join(" ");
            if args.len() > 90 {
                args.truncate(90);
                args.push('…');
            }
            let mut defs = String::new();
            for (name, f) in fields.iter().zip(&fs) {
                if let Some(m) = inner.defs.get(f)
                    && let Some(d) = m.get(pos)
                {
                    let mut val = v.render(d.value);
                    if val.len() > 60 {
                        val.truncate(60);
                        val.push('…');
                    }
                    defs.push_str(&format!(" {name}={val}{}", if d.provisional { "(prov)" } else { "" }));
                }
            }
            out.push_str(&format!(
                "  {:?} seg {:?} node {} {op} {:?} parent {:?} anchor {:?} slot {} [{args}]{defs}\n",
                pos.0,
                sid,
                nid.raw(),
                n.state(),
                n.parent.map(crate::api::NodeId::raw),
                n.anchor().map(crate::api::NodeId::raw),
                n.slot()
            ));
        }
    }
    eprint!("{out}");
}
