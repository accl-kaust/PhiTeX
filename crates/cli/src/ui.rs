//! The terminal chrome of the modern command line: colours on standard
//! error (a terminal, unless `NO_COLOR`; `CLICOLOR_FORCE` forces them), a
//! spinner while a build runs (a terminal only), lines printed above it,
//! the result line, and the engine's panics caught and shown as one line,
//! the whole message with `-v`. With `--json` none of it: one JSON object
//! a line on standard output ([`Ui::event`]).

use std::any::Any;
use std::io::{IsTerminal as _, Write as _};
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::session::Status;

/// Colours on standard error, unless it is no terminal or `NO_COLOR`.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
    /// Standard error is a terminal: symbols, and a spinner.
    pub tty: bool,
}

impl Style {
    pub fn detect() -> Style {
        let tty = std::io::stderr().is_terminal();
        let set = |v: &str| std::env::var_os(v).is_some_and(|x| !x.is_empty() && x != "0");
        Style {
            color: set("CLICOLOR_FORCE") || (tty && std::env::var_os("NO_COLOR").is_none()),
            tty,
        }
    }

    fn paint(self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_owned()
        }
    }

    pub fn red(self, s: &str) -> String {
        self.paint("1;31", s)
    }

    pub fn yellow(self, s: &str) -> String {
        self.paint("33", s)
    }

    pub fn green(self, s: &str) -> String {
        self.paint("32", s)
    }

    pub fn cyan(self, s: &str) -> String {
        self.paint("36", s)
    }

    pub fn bold(self, s: &str) -> String {
        self.paint("1", s)
    }

    pub fn dim(self, s: &str) -> String {
        self.paint("2", s)
    }

    /// `fancy` on a terminal, else `plain`.
    pub fn sym(self, fancy: &'static str, plain: &'static str) -> &'static str {
        if self.tty { fancy } else { plain }
    }
}

/// A duration for the terminal: milliseconds under a second.
pub fn ms(d: Duration) -> String {
    let m = d.as_secs_f64() * 1e3;
    if m >= 1000.0 {
        format!("{:.2} s", m / 1e3)
    } else {
        format!("{m:.1} ms")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `s` cut to `max` characters, an ellipsis where it is cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The terminal: what is printed, and the spinner.
pub struct Ui {
    pub st: Style,
    pub verbose: u8,
    pub quiet: bool,
    /// `--json`: events on standard output, nothing else.
    pub json: bool,
    /// The spinner's line, while one runs (a terminal only).
    spinning: Arc<Mutex<Option<String>>>,
}

/// A spinner while a build runs, until [`Spin::done`].
pub struct Spin {
    spinning: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    updater: Option<std::thread::JoinHandle<()>>,
}

/// The spinner's line: what is built, the session's phase, the time.
fn status_line(what: &str, s: &Status, elapsed: Duration) -> String {
    let mut parts = vec![what.to_owned()];
    let phase = s.phase();
    if !phase.is_empty() {
        parts.push(phase);
    }
    parts.push(format!("{:.1} s", elapsed.as_secs_f64()));
    parts.join(" · ")
}

const TICKS: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl Ui {
    pub fn new(st: Style, verbose: u8, quiet: bool, json: bool) -> Ui {
        Ui {
            st,
            verbose,
            quiet,
            json,
            spinning: Arc::new(Mutex::new(None)),
        }
    }

    /// A line on standard error, above the spinner if one runs.
    pub fn line(&self, s: &str) {
        if self.json {
            return;
        }
        let spin = self.spinning.lock().unwrap_or_else(PoisonError::into_inner);
        let mut e = std::io::stderr().lock();
        if spin.is_some() {
            let _ = write!(e, "\r\x1b[2K");
        }
        let _ = writeln!(e, "{s}");
    }

    /// `--json`: one event, a line on standard output.
    pub fn event(&self, v: &Value) {
        if self.json {
            let mut o = std::io::stdout().lock();
            let _ = writeln!(o, "{v}");
            let _ = o.flush();
        }
    }

    /// What the session says it is doing (`phitex: …`, dim), unless `-q`.
    pub fn note(&self, s: &str) {
        if !self.quiet {
            self.line(&self.st.dim(&format!("phitex: {s}")));
        }
        self.event(&json!({"event": "note", "message": s}));
    }

    pub fn error(&self, s: &str) {
        self.line(&format!("{}: {s}", self.st.red("error")));
        self.event(&json!({"event": "error", "message": s}));
    }

    pub fn warning(&self, s: &str) {
        self.line(&format!("{}: {s}", self.st.yellow("warning")));
        self.event(&json!({"event": "warning", "message": s}));
    }

    /// A spinner on standard error (a terminal only) showing `what` and
    /// what `status` says, until it is done.
    pub fn spin(&self, what: String, status: Arc<Status>) -> Spin {
        let stop = Arc::new(AtomicBool::new(false));
        let mut updater = None;
        if self.st.tty && !self.json {
            let (spinning, stop) = (self.spinning.clone(), stop.clone());
            let t0 = Instant::now();
            updater = Some(std::thread::spawn(move || {
                let mut k = 0;
                while !stop.load(Ordering::Relaxed) {
                    let line = format!(
                        "{} {}",
                        TICKS[k % TICKS.len()],
                        status_line(&what, &status, t0.elapsed())
                    );
                    {
                        let mut s = spinning.lock().unwrap_or_else(PoisonError::into_inner);
                        let _ = write!(std::io::stderr(), "\r\x1b[2K{line}");
                        *s = Some(line);
                    }
                    k += 1;
                    std::thread::sleep(Duration::from_millis(80));
                }
            }));
        }
        Spin {
            spinning: self.spinning.clone(),
            stop,
            updater,
        }
    }

    /// A build that failed inside the engine (a panic caught): one line
    /// with the message's first line, and where the whole of it is.
    pub fn failed(&self, what: &str, msg: &str) {
        let first = msg
            .lines()
            .next()
            .unwrap_or("the engine panicked without a message");
        self.error(&format!(
            "{what} failed inside the engine: {}",
            clip(first, 240)
        ));
        if self.verbose == 0 {
            self.line(&self.st.dim(
                "  (-v shows the whole message as it happens; PHITEX_SCHED=FILE writes the scheduler's trace)",
            ));
        }
    }

    /// The result line: `✓ paper.pdf · 12 pages · 1.2 s · 3 warnings`.
    pub fn result(&self, o: &Outcome) {
        let st = self.st;
        let sep = st.sym(" · ", ", ");
        let mut facts = Vec::new();
        if o.errors > 0 {
            facts.push(st.red(&plural(o.errors, "error", "errors")));
        }
        facts.push(plural(o.pages, "page", "pages"));
        if let Some(b) = o.bytes.filter(|_| self.verbose > 0) {
            facts.push(format!("{b} bytes"));
        }
        facts.push(st.bold(&ms(o.wall)));
        if o.warnings > 0 {
            facts.push(st.yellow(&plural(o.warnings, "warning", "warnings")));
        }
        let mut how = Vec::new();
        if let Some(e) = o.engine {
            how.push(e.to_owned());
        }
        if o.runs != 1 {
            how.push(plural(o.runs, "run", "runs"));
        }
        if !how.is_empty() {
            facts.push(st.dim(&how.join(", ")));
        }
        let subject = match o.output {
            Some(p) => st.bold(&p.display().to_string()),
            None => st.bold(o.file),
        };
        let mark = if o.errors > 0 {
            st.red(st.sym("✗", "failed:"))
        } else {
            st.green(st.sym("✓", "finished:"))
        };
        self.line(&format!("{mark} {subject}{sep}{}", facts.join(sep)));
    }
}

/// What a build came to, for [`Ui::result`].
pub struct Outcome<'a> {
    /// The output written (none: not written, or no pages).
    pub output: Option<&'a Path>,
    pub file: &'a str,
    pub pages: usize,
    pub bytes: Option<usize>,
    pub wall: Duration,
    pub errors: usize,
    pub warnings: usize,
    pub runs: usize,
    pub engine: Option<&'a str>,
}

impl Spin {
    pub fn done(self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(u) = self.updater {
            let _ = u.join();
            let mut s = self.spinning.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = write!(std::io::stderr(), "\r\x1b[2K");
            *s = None;
        }
    }
}

/// The panics this process met, first first (the engine's worker thread's
/// before the main thread's that follows it).
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn payload(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

/// Keep the panics for [`catch`] instead of printing them (a stalled
/// scheduler's dump is thousands of lines); `verbose` prints each as it
/// happens, whole.
pub fn capture_panics(verbose: bool) {
    std::panic::set_hook(Box::new(move |info| {
        let msg = payload(info.payload());
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        if verbose {
            eprintln!("panic at {at}:\n{msg}");
        }
        PANICS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{msg} (at {at})"));
    }));
}

/// `f`, a panic in it (the engine's: a build whose pieces did not settle)
/// an error instead of the process's end: the first panic's message.
pub fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    PANICS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let mut v = PANICS.lock().unwrap_or_else(PoisonError::into_inner);
        let first = v.first().cloned().unwrap_or_else(|| payload(&*p));
        v.clear();
        first
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_and_times() {
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
        assert_eq!(ms(Duration::from_millis(12)), "12.0 ms");
        assert_eq!(ms(Duration::from_millis(1500)), "1.50 s");
    }

    #[test]
    fn catches_panics() {
        capture_panics(false);
        assert_eq!(catch(|| 3).unwrap(), 3);
        let e = catch(|| -> u32 { panic!("the sequence of pieces did not settle") }).unwrap_err();
        assert!(
            e.starts_with("the sequence of pieces did not settle"),
            "{e}"
        );
    }
}
