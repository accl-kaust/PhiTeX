//! `PHITEX_RERUN_TRACE=1` (debug): each node's engine reads by run; a
//! node that runs again prints the reads whose values changed since its
//! previous run (what made it run again, when a value did).

use std::sync::{Mutex, OnceLock, PoisonError};

use phitex_defuse::{Field, FxHashMap, ValueId, Values};

type Reads = Vec<(String, Option<ValueId>, String)>;

struct Log {
    cur: FxHashMap<u64, Reads>,
    prev: FxHashMap<u64, Reads>,
    /// The fields each node that ran again published, in the order
    /// they ran again.
    again: Vec<(String, Vec<String>)>,
}

fn log() -> &'static Mutex<Log> {
    static L: OnceLock<Mutex<Log>> = OnceLock::new();
    L.get_or_init(|| {
        Mutex::new(Log {
            cur: FxHashMap::default(),
            prev: FxHashMap::default(),
            again: Vec::new(),
        })
    })
}

pub(crate) fn on() -> bool {
    crate::exec::env_flag!("PHITEX_RERUN_TRACE")
}

fn show(v: &Values, x: Option<ValueId>) -> String {
    x.map_or_else(
        || "(none)".to_owned(),
        |x| v.render(x).chars().take(100).collect(),
    )
}

/// Node `node` read field `f` as `x`.
pub(crate) fn read(v: &Values, node: u64, f: Field, x: Option<ValueId>) {
    let name = crate::val::field_name(v, f);
    let mut g = log().lock().unwrap_or_else(PoisonError::into_inner);
    let r = g.cur.entry(node).or_default();
    if !r.iter().any(|(n, _, _)| *n == name) {
        r.push((name, x, show(v, x)));
    }
}

/// Node `node` (its text `text`) finished a run.
pub(crate) fn finish(node: u64, op: &str, text: &str, published: Vec<String>) {
    let mut g = log().lock().unwrap_or_else(PoisonError::into_inner);
    let cur = g.cur.remove(&node).unwrap_or_default();
    if let Some(prev) = g.prev.get(&node) {
        // (the fields it read that a node run again before it published)
        let mut from = Vec::new();
        for (who, p) in &g.again {
            let hit: Vec<&str> = cur
                .iter()
                .filter(|(n, _, _)| p.contains(n))
                .map(|(n, _, _)| n.as_str())
                .take(6)
                .collect();
            if !hit.is_empty() {
                from.push(format!("{who}: {hit:?}"));
            }
        }
        let mut changed = Vec::new();
        for (n, x, s) in &cur {
            match prev.iter().find(|(m, _, _)| m == n) {
                Some((_, y, t)) if y != x => changed.push(format!("{n}: {t} -> {s}")),
                None => changed.push(format!("{n}: new read = {s}")),
                _ => {}
            }
        }
        eprintln!(
            "rerun: node {node} {op} {:?}: {} reads, {} changed",
            text.chars().take(60).collect::<String>(),
            cur.len(),
            changed.len()
        );
        let other: Vec<&str> = cur
            .iter()
            .filter(|(n, _, _)| n.starts_with('('))
            .map(|(n, _, _)| n.as_str())
            .collect();
        if !other.is_empty() {
            eprintln!("rerun:   also read {other:?}");
        }
        for f in from.iter().rev().take(3) {
            eprintln!(
                "rerun:   read what {}",
                f.chars().take(300).collect::<String>()
            );
        }
        for c in changed.iter().take(12) {
            eprintln!("rerun:   {}", c.chars().take(260).collect::<String>());
        }
    }
    if g.prev.contains_key(&node) {
        g.again.push((text.chars().take(40).collect(), published));
    }
    g.prev.insert(node, cur);
}

/// Node `node` read something that is not a field (`what`: a range, the
/// scope kind) as `x`.
pub(crate) fn note(v: &Values, node: u64, what: &str, x: Option<ValueId>) {
    let mut g = log().lock().unwrap_or_else(PoisonError::into_inner);
    let r = g.cur.entry(node).or_default();
    if !r.iter().any(|(n, _, _)| n == what) {
        r.push((what.to_owned(), x, show(v, x)));
    }
}
