//! `PARTEX_TEXPROF=report.txt` (feature `deps`): a TeX-level profiler.
//! Each macro call is charged the time until the next macro call: the
//! work its body's primitives do before handing on. Reports macros and
//! package families by time.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;

use partex_core::{Cell, Host, Tex, Tracker};

#[derive(Default)]
struct State {
    /// Per macro: calls and nanoseconds charged.
    by_cs: HashMap<i32, (u64, u64)>,
    last: i32,
    last_t: Option<std::time::Instant>,
    /// Per macro: calls by argument hash.
    args: HashMap<i32, HashMap<u64, u64>>,
}

/// The profiling tracker.
#[derive(Default)]
pub struct Profiler {
    s: RefCell<State>,
}

impl Tracker for Profiler {
    const PROFILE: bool = true;
    fn read(&self, _: Cell) {}
    fn write(&self, _: Cell) {}
    fn macro_call(&self, cs: i32) {
        let s = &mut *self.s.borrow_mut();
        let now = std::time::Instant::now();
        if let Some(t) = s.last_t {
            let ns = u64::try_from((now - t).as_nanos()).unwrap_or(u64::MAX);
            s.by_cs.entry(s.last).or_default().1 += ns;
        }
        s.by_cs.entry(cs).or_default().0 += 1;
        s.last = cs;
        s.last_t = Some(now);
    }
    fn macro_args(&self, cs: i32, hash: u64) {
        *self
            .s
            .borrow_mut()
            .args
            .entry(cs)
            .or_default()
            .entry(hash)
            .or_default() += 1;
    }
}

/// The package a macro belongs to, by its name.
fn family(n: &str) -> &'static str {
    let n = n.trim_start_matches('\\');
    if n.starts_with("pgfkeys") || n.starts_with("pgfk@") || n.starts_with("pgfqkeys") {
        "pgfkeys"
    } else if n.starts_with("pgfmath") {
        "pgfmath"
    } else if n.starts_with("pgfsys") || n.starts_with("pgf@sys") {
        "pgfsys"
    } else if n.starts_with("pgf") {
        "pgf (other)"
    } else if n.starts_with("tikz") {
        "tikz"
    } else if n.starts_with("__") || n.contains(':') {
        "expl3"
    } else if n.starts_with("lst") {
        "listings"
    } else if n.starts_with("Hy") || n.starts_with("hyper") {
        "hyperref"
    } else if n.starts_with("pgfmanual") || n.starts_with("codeexample") {
        "pgfmanual"
    } else {
        "LaTeX kernel and other"
    }
}

/// Write the report to `path`.
pub fn report<H: Host>(tex: &Tex<H, Profiler>, path: &std::path::Path) {
    let st = tex.tracker().s.take();
    let mut rows: Vec<(i32, u64, u64)> = st.by_cs.iter().map(|(&c, &(n, t))| (c, n, t)).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.2));
    let keys: Vec<Cell> = rows.iter().map(|r| Cell::Eqtb(r.0)).collect();
    let names: Vec<String> = tex
        .cell_names(&keys)
        .into_iter()
        .map(|n| String::from_utf8_lossy(&n).into_owned())
        .collect();
    let total: u64 = rows.iter().map(|r| r.2).sum();
    let ncalls: u64 = rows.iter().map(|r| r.1).sum();
    let pct = |t: u64| 100.0 * fl(t) / fl(total.max(1));
    let mut fam: HashMap<&str, (u64, u64)> = HashMap::new();
    for (r, n) in rows.iter().zip(&names) {
        let e = fam.entry(family(n)).or_default();
        e.0 += r.1;
        e.1 += r.2;
    }
    let mut fam: Vec<(&str, (u64, u64))> = fam.into_iter().collect();
    fam.sort_by_key(|f| std::cmp::Reverse(f.1.1));
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# {ncalls} macro calls, {:.2} s between the first and last",
        fl(total) / 1e9
    );
    let _ = writeln!(out, "# family: share calls ns/call");
    for (f, (n, t)) in &fam {
        let _ = writeln!(
            out,
            "{:6.2}% {n:11} {:7.0} {f}",
            pct(*t),
            fl(*t) / fl((*n).max(1))
        );
    }
    let _ = writeln!(out, "# macro: share calls ns/call");
    for (r, n) in rows.iter().zip(&names).take(400) {
        let _ = writeln!(
            out,
            "{:6.2}% {:11} {:7.0} {n}",
            pct(r.2),
            r.1,
            fl(r.2) / fl(r.1.max(1))
        );
    }
    // How repetitive the arguments of the most expensive macros are: the
    // calls a memo of (macro, arguments) could answer.
    let _ = writeln!(
        out,
        "# arguments: macro calls distinct-args calls-repeating top-arg-share"
    );
    for (r, n) in rows.iter().zip(&names).take(60) {
        let Some(a) = st.args.get(&r.0) else { continue };
        let calls: u64 = a.values().sum();
        let top = a.values().copied().max().unwrap_or(0);
        let _ = writeln!(
            out,
            "{:40} {calls:10} {:9} {:6.2}% {:6.2}%",
            n,
            a.len(),
            100.0 * fl(calls - fl_count(a.len())) / fl(calls.max(1)),
            100.0 * fl(top) / fl(calls.max(1))
        );
    }
    if let Err(e) = std::fs::write(path, out) {
        eprintln!("phitex: cannot write {}: {e}", path.display());
    }
}

/// For reporting: counts and nanoseconds far below 2^52.
#[allow(clippy::cast_precision_loss)]
fn fl(x: u64) -> f64 {
    x as f64
}

fn fl_count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
