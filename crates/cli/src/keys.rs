//! The watch's keys: on a terminal, standard input is read a key at a
//! time (no line editing, no echo; Ctrl-C comes as a key too, so the
//! watch puts the terminal back on its way out), and the width of the
//! terminal is known for the status line. Elsewhere (a pipe: an editor's
//! extension) a line at a time, as before.

use std::io::IsTerminal as _;
use std::sync::{Mutex, PoisonError};

#[cfg(unix)]
static SAVED: Mutex<Option<rustix::termios::Termios>> = Mutex::new(None);

/// Keys from now on: whether standard input (and standard error) is a
/// terminal that now gives them one at a time.
#[cfg(unix)]
pub fn raw() -> bool {
    use rustix::termios::{LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() || !std::io::stderr().is_terminal() {
        return false;
    }
    let Ok(was) = tcgetattr(&stdin) else {
        return false;
    };
    let mut t = was.clone();
    // (output untouched: lines still end as they do)
    t.local_modes
        .remove(LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG);
    t.special_codes[SpecialCodeIndex::VMIN] = 1;
    t.special_codes[SpecialCodeIndex::VTIME] = 0;
    if tcsetattr(&stdin, OptionalActions::Now, &t).is_err() {
        return false;
    }
    *SAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(was);
    true
}

#[cfg(not(unix))]
pub fn raw() -> bool {
    false
}

/// The terminal as it was before [`raw`].
pub fn restore() {
    #[cfg(unix)]
    if let Some(t) = SAVED.lock().unwrap_or_else(PoisonError::into_inner).take() {
        let _ =
            rustix::termios::tcsetattr(std::io::stdin(), rustix::termios::OptionalActions::Now, &t);
    }
}

/// The columns of the terminal on standard error.
pub fn width() -> Option<usize> {
    #[cfg(unix)]
    if let Ok(w) = rustix::termios::tcgetwinsize(std::io::stderr())
        && w.ws_col > 0
    {
        return Some(usize::from(w.ws_col));
    }
    None
}

/// This process's CPU time so far (all its threads) and resident memory
/// (Linux: `/proc/self/stat`; elsewhere none).
pub fn usage() -> Option<(std::time::Duration, u64)> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/stat").ok()?;
        // (after the command's `(…)`: state is field 3, utime 14, stime 15, rss 24)
        let f: Vec<&str> = s.get(s.rfind(')')? + 2..)?.split_whitespace().collect();
        let n = |k: usize| f.get(k - 3).and_then(|x| x.parse::<u64>().ok());
        let ticks = rustix::param::clock_ticks_per_second().max(1);
        let cpu = n(14)? + n(15)?;
        let cpu = std::time::Duration::from_micros(cpu * 1_000_000 / ticks);
        return Some((cpu, n(24)? * rustix::param::page_size() as u64));
    }
    #[allow(unreachable_code)]
    None
}

/// What a key asks of the watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Rebuild,
    Open,
    Problems,
    Warnings,
    Summary,
    Clear,
    Help,
    Quit,
    /// A key that means nothing here.
    Other(char),
}

impl Key {
    pub fn of(c: char) -> Option<Key> {
        Some(match c {
            'r' | 'R' | '\n' | '\r' => Key::Rebuild,
            'o' | 'O' => Key::Open,
            'p' | 'P' | 'e' => Key::Problems,
            'w' | 'W' => Key::Warnings,
            's' | 'S' => Key::Summary,
            'c' | 'C' | '\x0c' => Key::Clear,
            'h' | 'H' | '?' => Key::Help,
            'q' | 'Q' | '\x03' | '\x04' => Key::Quit,
            // (Ctrl-Z, escapes, spaces: nothing)
            c if c.is_control() || c == ' ' => return None,
            c => Key::Other(c),
        })
    }
}

/// The keys, at the status line's right end.
pub const BAR: &str = "h: keys";

/// The keys, for `h`.
pub const HELP: &[(&str, &str)] = &[
    ("r, Enter", "build again now"),
    ("o", "open the viewer (or the --open program on the output)"),
    ("p", "show the last build's errors again"),
    ("w", "the warnings, now and after every build (again: hide them)"),
    (
        "s",
        "how the build went (phases, reuse, parallelism, memory), now and after every build (again: hide it)",
    ),
    ("c", "clear the screen"),
    ("h, ?", "these keys"),
    ("q, Ctrl-C", "quit"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(Key::of('\n'), Some(Key::Rebuild));
        assert_eq!(Key::of('\x03'), Some(Key::Quit));
        assert_eq!(Key::of('x'), Some(Key::Other('x')));
        assert_eq!(Key::of('\x1b'), None);
    }
}
