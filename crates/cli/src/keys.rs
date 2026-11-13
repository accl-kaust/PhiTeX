//! The watch's keys: on a terminal, standard input is read a key at a
//! time (no line editing, no echo; Ctrl-C comes as a key too, so the
//! watch puts the terminal back on its way out), and the width of the
//! terminal is known for the status line. Elsewhere (a pipe: an editor's
//! extension) a line at a time, as before.

#[cfg(any(unix, windows))]
use std::io::IsTerminal as _;
use std::sync::atomic::{AtomicBool, Ordering};

/// The terminal as it was before [`raw`] and as [`raw`] made it, kept
/// where a signal handler can read them (set once, never moved).
#[cfg(unix)]
static MODES: std::sync::OnceLock<(libc::termios, libc::termios)> = std::sync::OnceLock::new();

/// The terminal is in the watch's mode now ([`raw`] until [`restore`]).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The terminal came back after a suspension (Ctrl-Z, `fg`): the bottom
/// lines are drawn again ([`resumed`]).
static RESUMED: AtomicBool = AtomicBool::new(false);

/// Keys from now on: whether standard input (and standard error) is a
/// terminal that now gives them one at a time. From here on every way
/// out puts the terminal back as it was: `q`, a quit by the code
/// ([`restore`]), a panic of the main thread (`main.rs`), and SIGTERM,
/// SIGHUP, SIGINT and SIGQUIT (the terminal put back, then the signal's
/// own end). Ctrl-Z (the key, or SIGTSTP) puts it back and suspends;
/// `fg` (SIGCONT) takes the keys again.
#[allow(
    unsafe_code,
    reason = "the terminal's modes and its signals: libc, no safe wrapper for a signal handler"
)]
#[cfg(unix)]
pub fn raw() -> bool {
    use std::os::fd::AsRawFd as _;
    let stdin = std::io::stdin();
    if !stdin.is_terminal() || !std::io::stderr().is_terminal() {
        return false;
    }
    let fd = stdin.as_raw_fd();
    // SAFETY: a termios is plain data; tcgetattr fills it.
    let mut was: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: fd is standard input, `was` a valid termios.
    if unsafe { libc::tcgetattr(fd, &raw mut was) } != 0 {
        return false;
    }
    let mut t = was;
    // (output untouched: lines still end as they do; Ctrl-C, Ctrl-Z and
    // Ctrl-\ come as keys, the watch's own to answer)
    t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
    t.c_cc[libc::VMIN] = 1;
    t.c_cc[libc::VTIME] = 0;
    // (a second watch session's call finds the terminal's own mode kept)
    let first = MODES.set((was, t)).is_ok();
    let Some((_, t)) = MODES.get() else {
        return false;
    };
    for sig in [
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTSTP,
        libc::SIGCONT,
    ] {
        handle(sig);
    }
    if first {
        // SAFETY: registering a function with no arguments for exit().
        unsafe {
            libc::atexit(at_exit);
        }
    }
    // SAFETY: fd is standard input, `t` a valid termios.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, t) } != 0 {
        return false;
    }
    ACTIVE.store(true, Ordering::SeqCst);
    true
}

/// Windows: the console's input without line editing, echo or Ctrl-C's
/// signal (`ENABLE_LINE_INPUT`, `ENABLE_ECHO_INPUT`,
/// `ENABLE_PROCESSED_INPUT` off): a key at a time, Ctrl-C as `\x03`.
#[cfg(windows)]
pub fn raw() -> bool {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return false;
    }
    if crossterm::terminal::enable_raw_mode().is_err() {
        return false;
    }
    ACTIVE.store(true, Ordering::SeqCst);
    true
}

#[cfg(not(any(unix, windows)))]
pub fn raw() -> bool {
    false
}

/// `sig` answered by [`on_signal`].
#[allow(
    unsafe_code,
    reason = "the terminal's modes and its signals: libc, no safe wrapper for a signal handler"
)]
#[cfg(unix)]
fn handle(sig: libc::c_int) {
    // SAFETY: the handler only makes async-signal-safe calls.
    unsafe {
        libc::signal(sig, on_signal as *const () as libc::sighandler_t);
    }
}

/// The terminal set to mode `t` (async-signal-safe).
#[allow(
    unsafe_code,
    reason = "the terminal's modes and its signals: libc, no safe wrapper for a signal handler"
)]
#[cfg(unix)]
fn set_mode(t: &libc::termios) {
    // SAFETY: tcsetattr is async-signal-safe; `t` is a valid termios.
    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, t);
    }
}

/// A signal while the terminal is the watch's: the terminal put back
/// first. SIGTSTP: suspended as without a handler, then, on `fg`, the
/// watch's mode again and the bottom lines redrawn. SIGCONT (a SIGSTOP
/// from elsewhere, then `fg`): the watch's mode again. Any other: the
/// signal's own end, as without a handler (its exit status 128 + the
/// signal, as a shell reports it).
#[allow(
    unsafe_code,
    reason = "the terminal's modes and its signals: libc, no safe wrapper for a signal handler"
)]
#[cfg(unix)]
extern "C" fn on_signal(sig: libc::c_int) {
    let Some((was, raw)) = MODES.get() else {
        return;
    };
    let active = ACTIVE.load(Ordering::SeqCst);
    match sig {
        libc::SIGCONT => {
            if active {
                set_mode(raw);
                RESUMED.store(true, Ordering::SeqCst);
            }
        }
        libc::SIGTSTP => {
            if active {
                set_mode(was);
            }
            // SAFETY: signal and raise are async-signal-safe.
            // (SIGTSTP is blocked while its handler runs: unblocked, so
            // the raise stops the process here, not after the handler
            // returns to find its own handler back)
            unsafe {
                libc::signal(libc::SIGTSTP, libc::SIG_DFL);
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&raw mut set);
                libc::sigaddset(&raw mut set, libc::SIGTSTP);
                libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const set, std::ptr::null_mut());
                libc::raise(libc::SIGTSTP);
                // (stopped here until `fg`)
                libc::signal(libc::SIGTSTP, on_signal as *const () as libc::sighandler_t);
            }
            if active {
                set_mode(raw);
                RESUMED.store(true, Ordering::SeqCst);
            }
        }
        _ => {
            if active {
                set_mode(was);
            }
            // SAFETY: signal and raise are async-signal-safe.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
                libc::raise(sig);
            }
        }
    }
}

/// `std::process::exit` from anywhere: the terminal put back.
#[cfg(unix)]
extern "C" fn at_exit() {
    restore();
}

/// Ctrl-Z typed: suspended as a shell's job is (the terminal put back
/// first, the watch's mode again on `fg`).
#[allow(
    unsafe_code,
    reason = "the terminal's modes and its signals: libc, no safe wrapper for a signal handler"
)]
pub fn suspend() {
    #[cfg(unix)]
    // SAFETY: raising a signal at this process.
    unsafe {
        libc::raise(libc::SIGTSTP);
    }
}

/// What a signal that ends the watch does ([`take_signals`]): the
/// watch's way out, as `q` takes it. None yet: the signal's own end.
type OnQuit = Box<dyn Fn() + Send + Sync>;

static ON_QUIT: std::sync::Mutex<Option<OnQuit>> = std::sync::Mutex::new(None);

/// The signal that ends the watch, once one came (0: none).
static SIGNALLED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// The signal that is ending the watch, if one is ([`take_signals`]).
pub fn signalled() -> Option<i32> {
    match SIGNALLED.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// The watch's way out for a signal that ends it ([`take_signals`]).
pub fn on_quit_signal(f: OnQuit) {
    *ON_QUIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(f);
}

/// SIGINT, SIGTERM and SIGHUP from now on taken by a thread of their
/// own, not by a handler (blocked in this thread, before any other is
/// started, and so in every thread it starts): the first ends the watch
/// as `q` does, through its way out ([`on_quit_signal`]: its files
/// written whole, the PDF file exact, the SyncTeX file of it), with the
/// signal's exit status (128 + the signal, as a shell reports it); a
/// second while that runs, or one before the watch has a way out, ends
/// the process at once (the terminal put back, then the signal's own
/// end). A program the watch starts gets them unblocked (`std::process`
/// resets the mask).
#[allow(
    unsafe_code,
    reason = "the signals blocked and waited for: libc, no safe wrapper"
)]
#[cfg(unix)]
pub fn take_signals() {
    // SAFETY: a sigset_t is plain data, filled by sigemptyset.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: `set` is a valid sigset_t; the signals are valid.
    unsafe {
        libc::sigemptyset(&raw mut set);
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaddset(&raw mut set, sig);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
    }
    let _ = std::thread::Builder::new()
        .name("phitex-signals".into())
        .spawn(move || {
            loop {
                let mut sig: libc::c_int = 0;
                // SAFETY: `set` is a valid sigset_t, `sig` an int.
                if unsafe { libc::sigwait(&raw const set, &raw mut sig) } != 0 {
                    continue;
                }
                let first = SIGNALLED
                    .compare_exchange(0, sig, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
                let quit = ON_QUIT
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if first && let Some(q) = quit.as_ref() {
                    q();
                    continue;
                }
                drop(quit);
                // (at once: the terminal put back, then the signal's own
                // end, raised at this thread, the one that has it
                // unblocked)
                restore();
                // SAFETY: the signal's default action restored, unblocked
                // in this thread and raised at it.
                unsafe {
                    libc::signal(sig, libc::SIG_DFL);
                    let mut one: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&raw mut one);
                    libc::sigaddset(&raw mut one, sig);
                    libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const one, std::ptr::null_mut());
                    libc::raise(sig);
                }
                std::process::exit(128 + sig);
            }
        });
}

/// Elsewhere: the signals' own ends.
#[cfg(not(unix))]
pub fn take_signals() {}

/// Whether the terminal came back from a suspension since last asked.
pub fn resumed() -> bool {
    RESUMED.swap(false, Ordering::SeqCst)
}

/// The terminal as it was before [`raw`] (a quit, a panic: any way out).
pub fn restore() {
    #[cfg(unix)]
    if ACTIVE.swap(false, Ordering::SeqCst)
        && let Some((was, _)) = MODES.get()
    {
        set_mode(was);
    }
    #[cfg(windows)]
    if ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Windows: escape sequences (colours, the status line) drawn by the
/// console, not printed (`ENABLE_VIRTUAL_TERMINAL_PROCESSING`; Windows
/// Terminal has it on already, the older console does not).
#[cfg(windows)]
pub fn console() {
    use std::io::IsTerminal as _;
    if std::io::stderr().is_terminal() || std::io::stdout().is_terminal() {
        let _ = crossterm::ansi_support::supports_ansi();
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
    #[cfg(windows)]
    if std::io::stderr().is_terminal()
        && let Ok((w, _)) = crossterm::terminal::size()
        && w > 0
    {
        return Some(usize::from(w));
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
    /// Ctrl-Z: suspended, as a shell's job.
    Suspend,
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
            'q' | 'Q' | '\x03' | '\x04' | '\x1c' => Key::Quit,
            '\x1a' => Key::Suspend,
            // (escapes, spaces: nothing)
            c if c.is_control() || c == ' ' => return None,
            c => Key::Other(c),
        })
    }
}

/// The keys, at the status line's right end.
pub const BAR: &str = "h: keys";

/// The keys, for `h`: each line short (it shares the terminal's width
/// with its key).
pub const HELP: &[(&str, &str)] = &[
    ("r, Enter", "build again now"),
    ("o", "open the viewer (or the --open program on the output)"),
    ("p", "show the last build's errors again"),
    ("w", "warnings, after every build (w again: off)"),
    ("s", "build stats, after every build (s again: off)"),
    ("c", "clear the screen"),
    ("h, ?", "these keys"),
    ("q, Ctrl-C", "quit"),
    ("Ctrl-Z", "suspend (fg resumes)"),
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
