//! The live area of the modern command line's terminal (DESIGN.md 2.5):
//! the line below everything printed that moves while a build runs, or
//! the footer of `partex watch`, drawn by a thread of its own.
//!
//! The build says what runs (a [`Task`]: a pass, a phase); the thread
//! samples the engine's progress board (`partex_core::progress`) and
//! draws about twelve times a second, never on the build's thread. What
//! is printed goes above the area: the area is cleared, the text written
//! and the area drawn again, in one write, as one frame (synchronized
//! output, which terminals without it ignore). The area is cut to the
//! terminal's width, so it never wraps and a redraw always finds it.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use partex_core::progress::{BOARD, Snapshot};

use crate::render::{Estimate, Settings, Style, compact, plural, secs};

/// After how long a task gets a live line (a quicker build never shows
/// one: its result is the first thing drawn).
const SHOW_AFTER: Duration = Duration::from_millis(150);

/// How often the live line is drawn while a task runs.
const TICK: Duration = Duration::from_millis(80);

/// What the live line shows while it runs: a pass, a phase.
pub struct Task {
    /// `Pass 1`, `Rebuilding`, `Linking`, …
    pub verb: String,
    /// What it does, if it says (`the saved build`).
    pub what: String,
    /// Whether the engine runs: its pages, commands and place are shown.
    pub engine: bool,
    /// The time shown counts from here (the build's start).
    pub started: Instant,
    /// When the task began.
    pub since: Instant,
    /// The board when the task began.
    pub base: Snapshot,
    /// How far a build from the start is, from the last one's totals.
    pub estimate: Option<Estimate>,
}

impl Task {
    /// A task beginning now, its time counted from `started`.
    pub fn new(
        verb: impl Into<String>,
        what: impl Into<String>,
        engine: bool,
        started: Instant,
    ) -> Self {
        Self {
            verb: verb.into(),
            what: what.into(),
            engine,
            started,
            since: Instant::now(),
            base: BOARD.snapshot(),
            estimate: None,
        }
    }
}

/// The footer of `partex watch`, below its log.
struct Footer {
    file: String,
    keys: bool,
}

/// What the render thread draws.
#[derive(Default)]
struct State {
    /// Bumped by every change (a frame composed before is not drawn).
    generation: u64,
    task: Option<Task>,
    footer: Option<Footer>,
    /// The last builds' times, oldest first.
    times: Vec<Duration>,
    quit: bool,
}

/// The lines below everything printed.
#[derive(Default)]
struct Screen {
    /// The generation of the state the area shows.
    generation: u64,
    area: Vec<String>,
}

/// Synchronized output: the terminal shows a frame whole.
const SYNC_ON: &str = "\x1b[?2026h";
const SYNC_OFF: &str = "\x1b[?2026l";

impl Screen {
    fn write(buf: &str) {
        let mut e = std::io::stderr().lock();
        let _ = e.write_all(buf.as_bytes());
        let _ = e.flush();
    }

    /// Back to the start of the area's first row.
    fn to_top(&self, buf: &mut String) {
        if self.area.len() > 1 {
            let _ = write!(buf, "\x1b[{}A", self.area.len() - 1);
        }
        if !self.area.is_empty() {
            buf.push('\r');
        }
    }

    fn push_area(buf: &mut String, area: &[String]) {
        for (i, l) in area.iter().enumerate() {
            if i > 0 {
                buf.push('\n');
            }
            buf.push_str(l);
            buf.push_str("\x1b[K");
        }
    }

    /// Print `text` above the area.
    fn print(&self, text: &str) {
        let mut buf = String::with_capacity(text.len() + 256);
        if !self.area.is_empty() {
            buf.push_str(SYNC_ON);
            self.to_top(&mut buf);
            buf.push_str("\x1b[J");
        }
        buf.push_str(text);
        if !text.ends_with('\n') {
            buf.push('\n');
        }
        if !self.area.is_empty() {
            Self::push_area(&mut buf, &self.area);
            buf.push_str(SYNC_OFF);
        }
        Self::write(&buf);
    }

    /// Show `area` in place of the area.
    fn draw(&mut self, area: Vec<String>) {
        if area == self.area {
            return;
        }
        let mut buf = String::from(SYNC_ON);
        self.to_top(&mut buf);
        Self::push_area(&mut buf, &area);
        buf.push_str("\x1b[J");
        buf.push_str(SYNC_OFF);
        Self::write(&buf);
        self.area = area;
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The live area of standard error.
pub struct Live {
    settings: Settings,
    state: Mutex<State>,
    wake: Condvar,
    screen: Mutex<Screen>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Live {
    #[must_use]
    pub fn new(settings: Settings) -> Arc<Self> {
        Arc::new(Self {
            settings,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            screen: Mutex::new(Screen::default()),
            thread: Mutex::new(None),
        })
    }

    /// The render thread, started the first time something is live;
    /// whether there is one (on a terminal).
    fn started(self: &Arc<Self>) -> bool {
        if !self.settings.progress {
            return false;
        }
        let mut t = lock(&self.thread);
        if t.is_none() {
            let live = self.clone();
            *t = std::thread::Builder::new()
                .name("partex-render".into())
                .spawn(move || live.run())
                .ok();
        }
        t.is_some()
    }

    /// Change the state, and have it drawn.
    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut st = lock(&self.state);
        f(&mut st);
        st.generation += 1;
        drop(st);
        self.wake.notify_all();
    }

    /// Clear the area now (a frame composed before is not drawn).
    fn clear(&self) {
        let generation = lock(&self.state).generation;
        let mut sc = lock(&self.screen);
        sc.generation = sc.generation.max(generation);
        sc.draw(Vec::new());
    }

    /// Print `text` (lines) above the area.
    pub fn print(&self, text: &str) {
        lock(&self.screen).print(text);
    }

    /// A dim line under the status lines.
    pub fn note(&self, text: &str) {
        let s = self.settings.style;
        self.print(&format!("{:>12} {}", "", s.dim(text)));
    }

    /// A status line, its verb in yellow (something to wait for).
    pub fn warn(&self, verb: &str, what: &str) {
        let s = self.settings.style;
        self.print(&format!("{} {what}", s.yellow(&format!("{verb:>12}"))));
    }

    /// The live line shows `task` (on a terminal).
    pub fn begin(self: &Arc<Self>, task: Task) {
        if self.started() {
            self.update(|st| st.task = Some(task));
        }
    }

    /// The running task's estimate is `e`.
    pub fn estimate(&self, e: Option<Estimate>) {
        self.update(|st| {
            if let Some(t) = &mut st.task {
                t.estimate = e;
            }
        });
    }

    /// The task ends: the footer comes back, else the area goes.
    pub fn end(&self) {
        self.update(|st| st.task = None);
        if lock(&self.state).footer.is_none() {
            self.clear();
        }
    }

    /// The task ends, and the area is cleared at once (for what is
    /// printed next).
    pub fn end_now(&self) {
        self.update(|st| st.task = None);
        self.clear();
    }

    /// A watch's footer below the log (on a terminal: whether it is).
    pub fn footer(self: &Arc<Self>, file: &str, keys: bool) -> bool {
        if !self.started() {
            return false;
        }
        self.update(|st| {
            st.task = None;
            st.footer = Some(Footer {
                file: file.to_owned(),
                keys,
            });
        });
        true
    }

    /// A build took `d` (the footer's sparkline).
    pub fn push_time(&self, d: Duration) {
        self.update(|st| {
            st.times.push(d);
            let n = st.times.len();
            if n > 16 {
                st.times.drain(..n - 16);
            }
        });
    }

    /// The area cleared for good (before the process exits).
    pub fn close(&self) {
        self.update(|st| {
            st.task = None;
            st.footer = None;
            st.quit = true;
        });
        self.clear();
    }

    /// The render thread stopped.
    pub fn stop(&self) {
        self.update(|st| st.quit = true);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }

    /// Ctrl-Z: the area cleared, the job stopped (`term::suspend`), and
    /// the area drawn again once it goes on.
    pub fn suspend(&self) {
        lock(&self.screen).draw(Vec::new());
        crate::term::suspend();
        self.update(|_| {});
    }

    /// The screen cleared, the area drawn again (`c`).
    pub fn clear_screen(&self) {
        let mut sc = lock(&self.screen);
        Screen::write("\x1b[H\x1b[2J\x1b[3J");
        let area = std::mem::take(&mut sc.area);
        sc.draw(area);
    }

    /// The render thread: a frame every [`TICK`] while a task runs, and
    /// one whenever the state changes.
    fn run(&self) {
        // (the state drawn last: a change made while a frame was drawn
        // is drawn next, not waited for)
        let mut seen = 0;
        let mut st = lock(&self.state);
        loop {
            if st.quit {
                return;
            }
            if st.generation == seen {
                st = if st.task.is_some() {
                    self.wake
                        .wait_timeout(st, TICK)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                } else {
                    self.wake
                        .wait(st)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                };
                if st.quit {
                    return;
                }
            }
            let generation = st.generation;
            seen = generation;
            let area = self.compose(&st);
            drop(st);
            let mut sc = lock(&self.screen);
            if sc.generation <= generation {
                sc.generation = generation;
                sc.draw(area);
            }
            drop(sc);
            st = lock(&self.state);
        }
    }

    /// The area for `st`.
    fn compose(&self, st: &State) -> Vec<String> {
        let (_, cols) = crate::term::size();
        let width = cols.saturating_sub(1).max(20);
        let now = Instant::now();
        let s = self.settings.style;
        let line = match (&st.task, &st.footer) {
            (Some(t), _) if now - t.started >= SHOW_AFTER => task_line(t, s, now, width),
            (_, Some(f)) => footer_line(f, &st.times, s, width),
            _ => return Vec::new(),
        };
        vec![crate::term::truncate(&line, width, s.unicode)]
    }
}

/// A duration on the live line, which changes ten times a second:
/// `0.4 s`, `12.3 s`, `2 min 05 s`.
fn ticking(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1} s")
    } else {
        secs(d)
    }
}

/// The spinner's frame after `t`.
fn spinner(t: Duration, s: Style) -> &'static str {
    const FANCY: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const ASCII: [&str; 4] = ["|", "/", "-", "\\"];
    let k = usize::try_from(t.as_millis() / 80).unwrap_or(0);
    if s.unicode {
        FANCY[k % FANCY.len()]
    } else {
        ASCII[k % ASCII.len()]
    }
}

/// A progress bar `cells` wide, `f` of it full.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn bar(f: f64, cells: usize, s: Style) -> String {
    let halves = (f.clamp(0.0, 1.0) * (cells * 2) as f64).round() as usize;
    let (full, half) = (halves / 2, halves % 2 == 1);
    if s.unicode {
        let mut done = "━".repeat(full);
        let mut rest = String::new();
        if half {
            done.push('╸');
        } else if full < cells {
            rest.push('╺');
        }
        let used = full + usize::from(half) + rest.chars().count();
        rest.push_str(&"━".repeat(cells.saturating_sub(used)));
        format!("{}{}", s.cyan(&done), s.dim(&rest))
    } else {
        let mut b = "=".repeat(full);
        if full < cells {
            b.push('>');
        }
        let pad = cells.saturating_sub(b.len());
        format!("[{b}{}]", " ".repeat(pad))
    }
}

/// A sparkline of `times` (oldest first), scaled between their least and
/// greatest on a logarithmic scale.
fn sparkline(times: &[Duration]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let ms: Vec<f64> = times
        .iter()
        .map(|t| t.as_secs_f64().max(1e-4).ln())
        .collect();
    let (lo, hi) = ms
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    ms.iter()
        .map(|&x| {
            let f = if hi - lo < 1e-9 {
                0.0
            } else {
                (x - lo) / (hi - lo)
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let k = (f * 7.0).round() as usize;
            BARS[k.min(7)]
        })
        .collect()
}

/// A file's name on the live line: the user's files as they are named,
/// the TeX tree's (absolute) by their base name.
fn short_name(name: &[u8]) -> String {
    let n = String::from_utf8_lossy(name);
    let n = n.strip_prefix("./").unwrap_or(&n);
    if n.starts_with('/') {
        n.rsplit('/').next().unwrap_or(n).to_owned()
    } else {
        n.to_owned()
    }
}

/// The live line of task `t`, at most `width` columns.
#[allow(clippy::cast_precision_loss)]
fn task_line(t: &Task, s: Style, now: Instant, width: usize) -> String {
    let board = BOARD.snapshot();
    let head = format!(
        "{} {}",
        s.cyan(&format!("{:>12}", t.verb)),
        s.cyan(spinner(now - t.since, s))
    );
    let elapsed = s.dim(&ticking(now - t.started));
    let mut facts: Vec<String> = Vec::new();
    if !t.what.is_empty() {
        facts.push(t.what.clone());
    }
    let mut fraction = None;
    if t.engine {
        let pages = board.pages.saturating_sub(t.base.pages);
        let commands = board.commands.saturating_sub(t.base.commands);
        if board.finishing {
            facts.push("finishing the PDF".to_owned());
        }
        if let Some(e) = t.estimate {
            fraction = Some((commands as f64 / e.commands as f64).min(0.99));
            if e.pages > 0 && pages > 0 {
                facts.push(format!("page {}/{}", pages.min(e.pages), e.pages));
            }
        } else if pages > 0 {
            facts.push(plural(usize::try_from(pages).unwrap_or(0), "page", "pages"));
        }
        if let Some(f) = &board.file
            && commands > 0
        {
            facts.push(format!("{}:{}", short_name(f), board.line));
        }
        if commands > 0 && fraction.is_none() {
            facts.push(format!("{} commands", compact(commands)));
        }
    }
    // (the time left: early on, mostly the last build's time; later,
    // mostly this one's rate)
    let left = fraction.zip(t.estimate).and_then(|(f, e)| {
        let ran = (now - t.since).as_secs_f64();
        if !(0.02..0.99).contains(&f) || ran < 0.5 {
            return None;
        }
        let total = (1.0 - f) * (e.millis as f64 / 1e3) + f * (ran / f);
        Some(Duration::from_secs_f64((total - ran).max(0.1)))
    });
    let tail = match left {
        Some(l) => format!(
            "{elapsed}{}",
            s.dim(&format!("{}{} left", s.sep(), ticking(l)))
        ),
        None => elapsed,
    };
    let sep = s.dim(s.sep());
    let fit = |facts: &[String], cells: usize| -> String {
        let mut line = head.clone();
        if let Some(f) = fraction
            && cells > 0
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let pct = (f * 100.0).floor() as u32;
            let _ = write!(line, " {} {pct:>2}%", bar(f, cells, s));
        }
        if !facts.is_empty() {
            line.push(' ');
            line.push_str(&facts.join(&sep));
        }
        let _ = write!(line, "  {tail}");
        line
    };
    // (the bar as wide as the terminal allows, the same from frame to
    // frame; then the facts that fit, dropped from the end; then no bar)
    let cells = match width {
        100.. => 24,
        70..100 => 16,
        _ => 10,
    };
    for cells in [cells, 0] {
        for keep in (0..=facts.len()).rev() {
            let line = fit(&facts[..keep], cells);
            if crate::term::width(&line) <= width {
                return line;
            }
        }
    }
    fit(&[], 0)
}

/// The footer of a watch, at most `width` columns: the keys go from the
/// end first, then the times.
fn footer_line(f: &Footer, times: &[Duration], s: Style, width: usize) -> String {
    let head = format!("{} {}", s.cyan(&format!("{:>12}", "Watching")), f.file);
    let sep = s.dim(s.sep());
    let mut facts = Vec::new();
    if let Some(last) = times.last() {
        let spark = if s.unicode && times.len() > 1 {
            format!("{} ", s.cyan(&sparkline(times)))
        } else {
            String::new()
        };
        facts.push(format!(
            "{spark}{}",
            s.dim(&format!("last {}", secs(*last)))
        ));
    }
    let keys = if f.keys {
        ["r rebuild", "o open", "q quit", "w warnings", "? help"]
            .iter()
            .map(|k| {
                let (key, what) = k.split_at(1);
                format!("{}{}", s.bold(key), s.dim(what))
            })
            .collect::<Vec<_>>()
    } else {
        vec![s.dim("q and Enter to quit")]
    };
    for with_times in [true, false] {
        for n in (0..=keys.len()).rev() {
            let mut parts: Vec<String> = Vec::new();
            if with_times {
                parts.extend(facts.iter().cloned());
            }
            if n > 0 {
                parts.push(keys[..n].join("  "));
            }
            let mut line = head.clone();
            if !parts.is_empty() {
                line.push_str(&sep);
                line.push_str(&parts.join(&sep));
            }
            if crate::term::width(&line) <= width {
                return line;
            }
        }
    }
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_sparklines() {
        let s = Style::plain();
        assert_eq!(bar(0.5, 10, s), "━━━━━╺━━━━");
        assert_eq!(bar(0.55, 10, s), "━━━━━╸━━━━");
        assert_eq!(bar(1.0, 4, s), "━━━━");
        let ascii = Style {
            unicode: false,
            ..s
        };
        assert_eq!(bar(0.5, 10, ascii), "[=====>    ]");
        let ms = |v: &[u64]| {
            v.iter()
                .map(|&m| Duration::from_millis(m))
                .collect::<Vec<_>>()
        };
        assert_eq!(sparkline(&ms(&[10, 100, 1000])), "▁▅█");
        assert_eq!(sparkline(&ms(&[20, 20])), "▁▁");
    }

    #[test]
    fn live_lines_fit_their_width() {
        let s = Style::plain();
        let mut t = Task::new("Pass 1", "", true, Instant::now());
        t.base = Snapshot::default();
        t.estimate = Some(Estimate {
            commands: 1000,
            pages: 12,
            millis: 1000,
        });
        for width in [40, 60, 80, 120] {
            let line = task_line(&t, s, t.since + Duration::from_secs(1), width);
            assert!(crate::term::width(&line) <= width, "{width}: {line}");
            assert!(line.starts_with("      Pass 1 "), "{line}");
        }
        let f = Footer {
            file: "paper.tex".into(),
            keys: true,
        };
        let times = [Duration::from_millis(18), Duration::from_millis(400)];
        assert_eq!(
            footer_line(&f, &times, s, 120),
            "    Watching paper.tex · ▁█ last 400 ms · r rebuild  o open  q quit  w warnings  ? help"
        );
        let narrow = footer_line(&f, &times, s, 60);
        assert!(crate::term::width(&narrow) <= 60, "{narrow}");
        assert!(narrow.contains("r rebuild"), "{narrow}");
        assert!(narrow.contains("last 400 ms"), "{narrow}");
    }
}
