//! A light trace (feature `ltrace`), on when `DEFUSE_LTRACE` names a file: the
//! reads each committed run rests on (its range reads, and its definition
//! reads of the fields `DEFUSE_LTRACE_FIELDS` names, comma separated; a
//! trailing `*` matches a prefix), contribution changes and touches, each
//! with a global sequence number. Each thread appends to its own buffer
//! (an uncontended lock); an evaluation's end writes them out, in sequence
//! order, appended to the file. Without the feature every hook is a
//! constant `false` test and compiles away; with it and the switch
//! unset, one relaxed load.
//!
//! Values are printed as their ids (content hashes: equal across runs
//! for equal values). Lines: `seq C node op` then `  R field from to value last` and
//! `  D field answer` per read; `seq K node field value|-`; `seq T node
//! cause`; `seq E evaluation-end`. Nodes are `NodeId`s.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::{Pred, Shared, Slot};
use crate::api::Cause;
use crate::values::{Field, ValueId};

struct Conf {
    path: String,
    names: Vec<String>,
    prefixes: Vec<String>,
}

static ON: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
static CONF: OnceLock<Option<Conf>> = OnceLock::new();
/// A thread's lines, with their sequence numbers.
type Buf = Arc<Mutex<Vec<(u64, String)>>>;

static BUFS: Mutex<Vec<Buf>> = Mutex::new(Vec::new());

thread_local! {
    static BUF: Buf = {
        let b = Arc::new(Mutex::new(Vec::new()));
        super::lock(&BUFS).push(b.clone());
        b
    };
}

/// Reads the switch (once per process).
pub(crate) fn init() {
    let c = CONF.get_or_init(|| {
        let path = std::env::var("DEFUSE_LTRACE").ok().filter(|p| !p.is_empty())?;
        let mut names = Vec::new();
        let mut prefixes = Vec::new();
        for n in std::env::var("DEFUSE_LTRACE_FIELDS").unwrap_or_default().split(',') {
            let n = n.trim();
            if let Some(p) = n.strip_suffix('*') {
                prefixes.push(p.to_string());
            } else if !n.is_empty() {
                names.push(n.to_string());
            }
        }
        Some(Conf { path, names, prefixes })
    });
    ON.store(c.is_some(), Ordering::Relaxed);
}

#[inline]
pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

fn emit(line: String) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    BUF.with(|b| super::lock(b).push((seq, line)));
}

fn field(sh: &Shared, f: Field) -> String {
    sh.values
        .with_str(f.0, str::to_string)
        .unwrap_or_else(|| format!("{:?}", f.0))
}

fn named(sh: &Shared, f: Field) -> bool {
    let Some(Some(c)) = CONF.get() else { return false };
    if c.names.is_empty() && c.prefixes.is_empty() {
        return false;
    }
    sh.values
        .with_str(f.0, |s| {
            c.names.iter().any(|n| n == s) || c.prefixes.iter().any(|p| s.starts_with(p.as_str()))
        })
        .unwrap_or(false)
}

/// A value as its id: a content hash, equal across runs for equal values
/// (rendering long sequences would change the timing it is to keep).
fn val(_sh: &Shared, x: ValueId) -> String {
    format!("{:x}", x.raw())
}

fn node(sh: &Shared, s: Slot) -> String {
    if s == super::NIL {
        "-".into()
    } else {
        format!("{}", sh.node_id(s).0)
    }
}

/// A run of `s` committed on the reads of `log`.
pub(crate) fn commit(sh: &Shared, s: Slot, log: &[Pred]) {
    let op = sh.op_name(sh.rec(s).data().op);
    let mut line = format!("C {} {op}", node(sh, s));
    for p in log {
        match p {
            Pred::Range {
                f,
                from,
                to,
                val: v,
                last,
                ..
            } => {
                line.push_str(&format!(
                    "\n  R {} {} {} {} {}",
                    field(sh, *f),
                    from.map_or_else(|| "-".into(), |x| x.to_string()),
                    to.map_or_else(|| "-".into(), |x| x.to_string()),
                    val(sh, *v),
                    node(sh, *last)
                ));
            }
            Pred::Def { f, ans, .. } if named(sh, *f) => {
                let a = match ans {
                    super::DAns::Site {
                        node: n,
                        val: super::DefVal::Val(x),
                        ..
                    } => {
                        format!("{} {}", node(sh, *n), val(sh, *x))
                    }
                    super::DAns::Site { node: n, val: v, .. } => format!("{} {v:?}", node(sh, *n)),
                    super::DAns::Env(x) | super::DAns::Seq(x) => {
                        format!("env {}", x.map_or_else(|| "-".into(), |x| val(sh, x)))
                    }
                    super::DAns::Loop(x) => format!("loop {}", val(sh, *x)),
                };
                line.push_str(&format!("\n  D {} {a}", field(sh, *f)));
            }
            _ => {}
        }
    }
    emit(line);
}

/// `s`'s contribution to `f` became `x` (`None`: withdrawn).
pub(crate) fn contrib(sh: &Shared, s: Slot, f: Field, x: Option<ValueId>) {
    emit(format!(
        "K {} {} {}",
        node(sh, s),
        field(sh, f),
        x.map_or_else(|| "-".into(), |x| val(sh, x))
    ));
}

/// `s` was touched.
pub(crate) fn touch(sh: &Shared, s: Slot, cause: &Cause) {
    emit(format!("T {} {cause:?}", node(sh, s)));
}

/// The evaluation's end: every thread's lines so far, appended in order.
pub(crate) fn flush() {
    let Some(Some(c)) = CONF.get() else { return };
    emit("E".into());
    let mut all: Vec<(u64, String)> = Vec::new();
    for b in super::lock(&BUFS).iter() {
        all.append(&mut super::lock(b));
    }
    all.sort_unstable_by_key(|e| e.0);
    let mut text = String::new();
    for (seq, l) in all {
        text.push_str(&format!("{seq} {l}\n"));
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&c.path) {
        let _ = f.write_all(text.as_bytes());
    }
}
