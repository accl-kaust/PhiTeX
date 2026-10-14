//! `PHITEX_RERUN_TRACE=1` (debug): each node's engine reads by run; a
//! node that runs again prints the reads whose values changed since its
//! previous run (what made it run again, when a value did).

use std::sync::{Mutex, OnceLock, PoisonError};

use phitex_defuse::{Field, FxHashMap, ValueId, Values};

type Reads = Vec<(String, Option<ValueId>, String)>;

struct Log {
    cur: FxHashMap<u64, Reads>,
    prev: FxHashMap<u64, Reads>,
}

fn log() -> &'static Mutex<Log> {
    static L: OnceLock<Mutex<Log>> = OnceLock::new();
    L.get_or_init(|| {
        Mutex::new(Log {
            cur: FxHashMap::default(),
            prev: FxHashMap::default(),
        })
    })
}

pub(crate) fn on() -> bool {
    crate::exec::env_flag!("PHITEX_RERUN_TRACE")
}

fn show(v: &Values, x: Option<ValueId>) -> String {
    x.map_or_else(|| "(none)".to_owned(), |x| v.render(x).chars().take(100).collect())
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
pub(crate) fn finish(node: u64, op: &str, text: &str) {
    let mut g = log().lock().unwrap_or_else(PoisonError::into_inner);
    let cur = g.cur.remove(&node).unwrap_or_default();
    if let Some(prev) = g.prev.get(&node) {
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
        for c in changed.iter().take(12) {
            eprintln!("rerun:   {}", c.chars().take(260).collect::<String>());
        }
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
