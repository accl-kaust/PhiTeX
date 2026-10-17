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

/// `s`'s width on a terminal (its characters without the colours).
pub fn width(s: &str) -> usize {
    plain(s).chars().count()
}

/// `s` padded with spaces to `w` columns (on the left with `right`).
pub fn pad(s: &str, w: usize, right: bool) -> String {
    let fill = " ".repeat(w.saturating_sub(width(s)));
    if right {
        format!("{fill}{s}")
    } else {
        format!("{s}{fill}")
    }
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

/// Every message, besides the terminal: the viewer's console. Kept until
/// a sink is set (the messages before the viewer started), then sent to
/// it as they come.
#[derive(Default)]
struct Tap {
    sink: Option<Box<dyn Fn(Value) + Send>>,
    early: Vec<Value>,
}

impl Tap {
    fn tell(&mut self, v: Value) {
        match &self.sink {
            Some(f) => f(v),
            None if self.early.len() < 2000 => self.early.push(v),
            None => {}
        }
    }
}

/// The terminal's last lines, below what is printed: the build's live
/// tree while it runs, else the watch's status line (a terminal only).
#[derive(Default)]
struct Bottom {
    spin: Vec<String>,
    idle: Option<String>,
    /// The keys shown at the status line's right end.
    keys: Option<&'static str>,
    /// The lines on the screen now.
    drawn: usize,
    /// Nothing drawn below any more (the watch is quitting).
    gone: bool,
}

impl Bottom {
    fn shown(&self) -> &[String] {
        if self.spin.is_empty() {
            self.idle.as_slice()
        } else {
            &self.spin
        }
    }

    /// The lines on the screen erased, the cursor at the first's start.
    fn erase(&mut self, e: &mut impl std::io::Write) {
        if self.drawn > 0 {
            let _ = write!(e, "\r\x1b[2K");
            for _ in 1..self.drawn {
                let _ = write!(e, "\x1b[1A\x1b[2K");
            }
            self.drawn = 0;
        }
    }
}

/// The terminal: what is printed, and the spinner.
#[derive(Clone)]
pub struct Ui {
    pub st: Style,
    pub verbose: u8,
    pub quiet: bool,
    /// `--json`: events on standard output, nothing else.
    pub json: bool,
    bottom: Arc<Mutex<Bottom>>,
    tap: Arc<Mutex<Tap>>,
}

/// A spinner while a build runs, until [`Spin::done`].
pub struct Spin {
    bottom: Arc<Mutex<Bottom>>,
    tap: Arc<Mutex<Tap>>,
    stop: Arc<AtomicBool>,
    updater: Option<std::thread::JoinHandle<()>>,
}

/// The live tree under the spinner's line, once a build has run a
/// while: each phase ended with its time, the one running with its time
/// so far, the cores busy and the memory held.
fn tree(st: Style, s: &Status, tick: char, load: Option<(f64, u64)>, w: usize) -> Vec<String> {
    let (done, now) = s.phases();
    let n = done.len() + usize::from(now.is_some());
    let mut out = Vec::with_capacity(n);
    let branch = |k: usize| if k + 1 == n { "└─" } else { "├─" };
    for (k, (name, d)) in done.iter().enumerate() {
        out.push(format!(
            "  {} {} {} {}",
            st.dim(branch(k)),
            st.green("✓"),
            pad(name, 14, false),
            st.dim(&pad(&ms(*d), 9, true))
        ));
    }
    if let Some((name, d)) = now {
        let mut l = format!(
            "  {} {} {} {}",
            st.dim(branch(done.len())),
            st.cyan(&tick.to_string()),
            pad(&name, 14, false),
            pad(&ms(d), 9, true)
        );
        if let Some((cores, rss)) = load {
            let mem = bytesize::ByteSize::b(rss).display().si().to_string();
            l.push_str(&st.dim(&format!("   {cores:.1} cores busy · {mem}")));
        }
        out.push(l);
    }
    // (under the engine: the jobs done of those known, and what each worker
    // runs now, alike ones counted together)
    if let Some(live) = s.live() {
        let p = &live.progress;
        let (done, total) = (p.done(), p.total());
        let mut rows: Vec<(String, usize)> = Vec::new();
        let mut idle = 0;
        for slot in p.running() {
            let Some((op, at)) = slot else {
                idle += 1;
                continue;
            };
            let what = match op {
                1 => "text",
                2 => "box",
                3 => "page",
                _ => "other",
            };
            let place = if at == phitex_tex::Progress::INSIDE {
                "inside a macro".to_owned()
            } else {
                let line = live.line_of(at);
                format!("{}:{line}", live.file)
            };
            let row = format!("{} {place}", pad(what, 6, false));
            match rows.iter_mut().find(|r| r.0 == row) {
                Some(r) => r.1 += 1,
                None => rows.push((row, 1)),
            }
        }
        out.push(format!(
            "       {}",
            st.dim(&format!(
                "[{done}/{total}] jobs · {} running · {idle} idle",
                rows.iter().map(|r| r.1).sum::<usize>()
            ))
        ));
        let shown = rows.len().min(6);
        for (k, (row, n)) in rows.iter().take(shown).enumerate() {
            let branch = if k + 1 == shown { "└─" } else { "├─" };
            let times = if *n > 1 {
                format!(" ×{n}")
            } else {
                String::new()
            };
            out.push(format!("       {} {row}{}", st.dim(branch), st.dim(&times)));
        }
    }
    // (cut to the terminal: a wrapped line would break the redraw)
    out.into_iter()
        .map(|l| {
            if width(&l) > w {
                clip(&plain(&l), w)
            } else {
                l
            }
        })
        .collect()
}

/// The spinner's line: what is built, the session's phase, the time, and
/// (`load`) the cores busy lately and the memory held.
fn status_line(what: &str, s: &Status, elapsed: Duration, load: Option<(f64, u64)>) -> String {
    let mut parts = vec![what.to_owned()];
    let phase = s.phase();
    if !phase.is_empty() {
        parts.push(phase);
    }
    parts.push(format!("{:.1} s", elapsed.as_secs_f64()));
    if let Some((cores, rss)) = load {
        parts.push(format!("{cores:.1} cores busy"));
        parts.push(bytesize::ByteSize::b(rss).display().si().to_string());
    }
    parts.join(" · ")
}

/// The cores busy since the last sample, and the memory now.
struct Load {
    last: Option<(Instant, Duration)>,
    now: Option<(f64, u64)>,
}

impl Load {
    fn sample(&mut self) {
        let Some((cpu, rss)) = crate::keys::usage() else {
            return;
        };
        let t = Instant::now();
        match self.last {
            Some((t0, c0)) => {
                let dt = t.duration_since(t0).as_secs_f64();
                if dt >= 0.4 {
                    let cores = cpu.saturating_sub(c0).as_secs_f64() / dt;
                    self.now = Some((cores, rss));
                    self.last = Some((t, cpu));
                }
            }
            None => self.last = Some((t, cpu)),
        }
    }
}

const TICKS: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// `s` without its colours.
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            // (`ESC [ … m`)
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The bottom lines written again, over what they were.
fn redraw(e: &mut impl std::io::Write, b: &mut Bottom) {
    b.erase(e);
    if b.gone {
        let _ = e.flush();
        return;
    }
    let lines = b.shown();
    let _ = write!(e, "{}", lines.join("\n"));
    b.drawn = lines.len();
    let _ = e.flush();
}

impl Ui {
    pub fn new(st: Style, verbose: u8, quiet: bool, json: bool) -> Ui {
        Ui {
            st,
            verbose,
            quiet,
            json,
            bottom: Arc::default(),
            tap: Arc::default(),
        }
    }

    /// Every message from now on (and those so far) also to `sink`, as
    /// `{"kind": …, "text": …}` (the text without colours) or, for
    /// [`Ui::event`]'s, `{"kind": "event", "event": {…}}`.
    pub fn tap(&self, sink: Box<dyn Fn(Value) + Send>) {
        let mut t = lock(&self.tap);
        for v in t.early.drain(..) {
            sink(v);
        }
        t.sink = Some(sink);
    }

    fn tell(&self, kind: &str, text: &str) {
        lock(&self.tap).tell(json!({"kind": kind, "text": plain(text)}));
    }

    /// `s` on standard error, above the bottom line.
    fn print(&self, s: &str) {
        if self.json {
            return;
        }
        let mut b = lock(&self.bottom);
        let mut e = std::io::stderr().lock();
        let shown = b.drawn > 0;
        b.erase(&mut e);
        let _ = writeln!(e, "{s}");
        if shown {
            redraw(&mut e, &mut b);
        }
    }

    /// A line on standard error, above the spinner if one runs.
    pub fn line(&self, s: &str) {
        self.print(s);
        self.tell("line", s);
    }

    /// `--json`: one event, a line on standard output.
    pub fn event(&self, v: &Value) {
        if self.json {
            let mut o = std::io::stdout().lock();
            let _ = writeln!(o, "{v}");
            let _ = o.flush();
        }
        lock(&self.tap).tell(json!({"kind": "event", "event": v}));
    }

    /// What the session says (`note: …`), unless `-q`.
    pub fn note(&self, s: &str) {
        if !self.quiet {
            self.print(&format!("{}: {s}", self.st.yellow("note")));
        }
        self.tell("note", s);
        self.event_only(&json!({"event": "note", "message": s}));
    }

    pub fn error(&self, s: &str) {
        self.print(&format!("{}: {s}", self.st.red("error")));
        self.tell("error", s);
        self.event_only(&json!({"event": "error", "message": s}));
    }

    pub fn warning(&self, s: &str) {
        self.print(&format!("{}: {s}", self.st.yellow("warning")));
        self.tell("warning", s);
        self.event_only(&json!({"event": "warning", "message": s}));
    }

    /// `--json`'s event, not told again (its line was).
    fn event_only(&self, v: &Value) {
        if self.json {
            let mut o = std::io::stdout().lock();
            let _ = writeln!(o, "{v}");
            let _ = o.flush();
        }
    }

    /// The watch's status, below what is printed while no build runs (a
    /// terminal only; none: no line), and told: `ok` its mark (none: dim).
    pub fn status(&self, text: Option<&str>, ok: Option<bool>) {
        let st = self.st;
        if st.tty && !self.json {
            let mut b = lock(&self.bottom);
            b.idle = text.map(|t| {
                let w = crate::keys::width().unwrap_or(80).saturating_sub(1);
                let mark = match ok {
                    Some(true) => st.green("●"),
                    Some(false) => st.red("●"),
                    None => st.dim("●"),
                };
                // (the keys at the right end, when there is room)
                let keys = b.keys.unwrap_or("");
                let room = w.saturating_sub(2);
                let fits = !keys.is_empty() && t.chars().count() + 3 + keys.len() <= room;
                if fits {
                    let gap = room - t.chars().count() - keys.len();
                    format!("{mark} {}{}{}", st.dim(t), " ".repeat(gap), st.dim(keys))
                } else {
                    format!("{mark} {}", st.dim(&clip(t, room)))
                }
            });
            if b.spin.is_empty() {
                redraw(&mut std::io::stderr().lock(), &mut b);
            }
        }
        lock(&self.tap).tell(json!({"kind": "status", "text": text.unwrap_or(""), "ok": ok}));
    }

    /// The bottom lines erased for good, a build's tree too (the watch
    /// quits).
    pub fn finish(&self) {
        if self.st.tty && !self.json {
            let mut b = lock(&self.bottom);
            b.gone = true;
            redraw(&mut std::io::stderr().lock(), &mut b);
        }
    }

    /// `keys` at the status line's right end from now on.
    pub fn keys(&self, keys: &'static str) {
        lock(&self.bottom).keys = Some(keys);
    }

    /// The screen cleared, the bottom lines again.
    pub fn clear(&self) {
        if self.st.tty && !self.json {
            let mut b = lock(&self.bottom);
            let mut e = std::io::stderr().lock();
            let _ = write!(e, "\x1b[2J\x1b[H");
            b.drawn = 0;
            redraw(&mut e, &mut b);
        }
    }

    /// A spinner on standard error (a terminal only) showing `what` and
    /// what `status` says, until it is done; the viewer is told it began.
    pub fn spin(&self, what: String, status: Arc<Status>) -> Spin {
        lock(&self.tap).tell(json!({"kind": "busy", "text": what}));
        let stop = Arc::new(AtomicBool::new(false));
        // (the line drawn on a terminal; told to the viewer every second)
        let draw = self.st.tty && !self.json;
        let (bottom, tap, stopping) = (self.bottom.clone(), self.tap.clone(), stop.clone());
        let st = self.st;
        let t0 = Instant::now();
        let updater = Some(std::thread::spawn(move || {
            let mut k = 0usize;
            let mut load = Load {
                last: None,
                now: None,
            };
            while !stopping.load(Ordering::Relaxed) {
                if k.is_multiple_of(6) {
                    load.sample();
                }
                let text = status_line(&what, &status, t0.elapsed(), load.now);
                if draw {
                    let w = crate::keys::width().unwrap_or(80).saturating_sub(1);
                    let tick = TICKS[k % TICKS.len()];
                    let elapsed = t0.elapsed();
                    let mut lines = vec![format!(
                        "{} {}",
                        st.cyan(&tick.to_string()),
                        clip(&text, w.saturating_sub(2))
                    )];
                    // (a quick build shows only the spinner: a tree that
                    // flashed would be noise)
                    if elapsed >= Duration::from_millis(300) {
                        lines[0] = format!(
                            "{} {}",
                            st.cyan(&tick.to_string()),
                            pad(
                                &clip(&what, w.saturating_sub(12)),
                                w.saturating_sub(11),
                                false
                            ) + &st.bold(&pad(&format!("{:.1} s", elapsed.as_secs_f64()), 9, true))
                        );
                        lines.extend(tree(st, &status, tick, load.now, w));
                    }
                    let mut b = lock(&bottom);
                    b.spin = lines;
                    redraw(&mut std::io::stderr().lock(), &mut b);
                }
                if k % 12 == 11 {
                    lock(&tap).tell(json!({"kind": "busy", "text": text}));
                }
                k += 1;
                // (woken at once when the build is done: its end does not
                // wait for the next tick)
                std::thread::park_timeout(Duration::from_millis(80));
            }
        }));
        Spin {
            bottom: self.bottom.clone(),
            tap: self.tap.clone(),
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
        if o.diagnostics > 0 {
            facts.push(st.dim(&plural(o.diagnostics, "diagnostic", "diagnostics")));
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
    /// The engine's own diagnostics (not TeX's errors).
    pub diagnostics: usize,
    pub runs: usize,
    pub engine: Option<&'a str>,
}

impl Spin {
    pub fn done(self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(u) = self.updater {
            u.thread().unpark();
            let _ = u.join();
            let mut b = lock(&self.bottom);
            if !b.spin.is_empty() {
                b.spin.clear();
                redraw(&mut std::io::stderr().lock(), &mut b);
            }
        }
        lock(&self.tap).tell(json!({"kind": "busy", "text": ""}));
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
            // (with RUST_BACKTRACE set, where it came from)
            let bt = std::backtrace::Backtrace::capture();
            if bt.status() == std::backtrace::BacktraceStatus::Captured {
                eprintln!("{bt}");
            }
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
    fn plain_text() {
        assert_eq!(plain("\x1b[1;31merror\x1b[0m: x"), "error: x");
    }

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
