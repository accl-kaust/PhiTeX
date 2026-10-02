//! `PARTEX_TIMELINE=file.json`: a timeline of a process's coarse phases
//! (builds and passes, checkpoints kept, saved and loaded, splices,
//! native tools), written at exit as Chrome trace JSON for Perfetto or
//! `chrome://tracing` (DESIGN.md §8). Off, a phase costs one load and a
//! branch.

use std::cell::Cell;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Event {
    name: &'static str,
    detail: String,
    tid: u32,
    /// Microseconds since the first phase, and the phase's length.
    start: f64,
    dur: f64,
}

struct Recorder {
    path: std::path::PathBuf,
    epoch: Instant,
    events: Mutex<Vec<Event>>,
}

/// The timeline's file when the command line asks for one (`partex
/// trace`), over `PARTEX_TIMELINE`.
static PATH: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Record a timeline into `path` (before the first phase begins).
pub fn record_to(path: std::path::PathBuf) {
    let _ = PATH.set(path);
}

fn recorder() -> Option<&'static Recorder> {
    static R: OnceLock<Option<Recorder>> = OnceLock::new();
    R.get_or_init(|| {
        let path = PATH.get().map(|p| p.clone().into_os_string());
        path.or_else(|| std::env::var_os("PARTEX_TIMELINE"))
            .map(|p| Recorder {
                path: p.into(),
                epoch: Instant::now(),
                events: Mutex::new(Vec::new()),
            })
    })
    .as_ref()
}

/// This thread's number in the timeline.
fn tid() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    thread_local!(static TID: Cell<u32> = const { Cell::new(0) });
    TID.with(|t| {
        if t.get() == 0 {
            t.set(NEXT.fetch_add(1, Ordering::Relaxed));
        }
        t.get()
    })
}

/// A phase, recorded when dropped.
pub struct Phase {
    name: &'static str,
    detail: String,
    start: Instant,
}

impl Phase {
    /// Say more about the phase (shown as its argument).
    pub fn detail(&mut self, detail: impl Into<String>) {
        self.detail = detail.into();
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        let Some(r) = recorder() else { return };
        let end = Instant::now();
        let start = self.start.duration_since(r.epoch).as_secs_f64() * 1e6;
        let dur = end.duration_since(self.start).as_secs_f64() * 1e6;
        if let Ok(mut e) = r.events.lock() {
            e.push(Event {
                name: self.name,
                detail: std::mem::take(&mut self.detail),
                tid: tid(),
                start,
                dur,
            });
        }
    }
}

/// Begin phase `name` (`None` when no timeline is recorded).
pub fn phase(name: &'static str) -> Option<Phase> {
    recorder()?;
    Some(Phase {
        name,
        detail: String::new(),
        start: Instant::now(),
    })
}

/// A JSON string literal.
pub fn quoted(s: &str) -> String {
    let mut q = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(q, "\\u{:04x}", u32::from(c));
            }
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// Write the timeline, if one is recorded (before the process exits).
pub fn finish() {
    let Some(r) = recorder() else { return };
    let events = r
        .events
        .lock()
        .map(|mut e| std::mem::take(&mut *e))
        .unwrap_or_default();
    let mut out = String::from("{\"traceEvents\":[\n");
    for (i, e) in events.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}{{\"name\":{},\"ph\":\"X\",\"pid\":1,\"tid\":{},\"ts\":{:.1},\"dur\":{:.1},\"args\":{{\"detail\":{}}}}}",
            if i == 0 { "" } else { "," },
            quoted(e.name),
            e.tid,
            e.start,
            e.dur,
            quoted(&e.detail)
        );
    }
    out.push_str("]}\n");
    if let Err(e) = std::fs::write(&r.path, out) {
        eprintln!("partex: can't write the timeline {}: {e}", r.path.display());
    }
}
