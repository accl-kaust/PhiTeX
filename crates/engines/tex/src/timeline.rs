//! One edit's phases from its event to its bytes written, as a table
//! (`PHITEX_TEX_TIME`): the watch begins it at the edit's event, each
//! step on the way marks the end of its phase ([`lap`]), work counted
//! inside a phase on other threads is noted beside it ([`note`]), and
//! the watch prints it when the event goes out ([`end`]). A build no
//! edit began (the cold one) marks nothing.

use std::fmt::Write as _;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

struct Line {
    start: Instant,
    last: Instant,
    /// (phase, time, a note within the phase after it rather than a lap)
    rows: Vec<(String, Duration, bool)>,
}

static LINE: Mutex<Option<Line>> = Mutex::new(None);

fn with(f: impl FnOnce(&mut Line)) {
    let mut g = LINE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(l) = g.as_mut() {
        f(l);
    }
}

/// An edit's timeline begins at `t`, its event (with `PHITEX_TEX_TIME`).
pub fn begin_at(t: Instant) {
    if !crate::exec::env_flag!("PHITEX_TEX_TIME") {
        return;
    }
    *LINE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Line {
        start: t,
        last: t,
        rows: Vec::new(),
    });
}

/// Whether a timeline is going on.
pub fn active() -> bool {
    LINE.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// The phase `name` ends now (its time: since the last phase ended).
pub fn lap(name: &str) {
    with(|l| {
        let now = Instant::now();
        l.rows.push((name.to_owned(), now - l.last, false));
        l.last = now;
    });
}

/// Within the phase going on: `name` took `d` (other threads' work, or
/// a part of it), shown under that phase.
pub fn note(name: &str, d: Duration) {
    with(|l| l.rows.push((name.to_owned(), d, true)));
}

/// The timeline printed (phases of 0.3 ms or more, and the rest summed)
/// and ended.
pub fn end(label: &str) {
    let Some(l) = LINE.lock().unwrap_or_else(PoisonError::into_inner).take() else {
        return;
    };
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let min = Duration::from_micros(300);
    let total = l.last - l.start;
    let mut out = format!(
        "edit timeline ({label}): {:.2} ms from the event\n",
        ms(total)
    );
    let mut small = (Duration::ZERO, 0usize);
    // (a note is shown under the phase it was made in: the next lap)
    let mut pending = String::new();
    for (name, d, is_note) in &l.rows {
        if *is_note {
            if *d >= min {
                let _ = writeln!(pending, "               . {name} {:.2} ms", ms(*d));
            }
            continue;
        }
        if *d >= min {
            let _ = writeln!(out, "  {:>9.2} ms  {name}", ms(*d));
        } else {
            small.0 += *d;
            small.1 += 1;
        }
        out.push_str(&pending);
        pending.clear();
    }
    out.push_str(&pending);
    let _ = write!(
        out,
        "  {:>9.2} ms  ({} phases under 0.3 ms)",
        ms(small.0),
        small.1
    );
    eprintln!("{out}");
}
