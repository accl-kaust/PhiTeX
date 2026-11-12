//! The terminal chrome of the modern command line: colours on standard
//! error (a terminal, unless `NO_COLOR`; `CLICOLOR_FORCE` forces them), a
//! spinner while a build runs (a terminal only), lines printed above it,
//! the result line, and the engine's panics caught and shown as one line,
//! the whole message with `-v`. With `--json` none of it: one JSON object
//! a line on standard output ([`Ui::event`]).

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub use phitex_live::panics::{capture_panics, catch};

use crate::session::Status;

/// `--color`: always (true) or never (false); none, by the stream.
static COLOR: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();

/// `--color`'s choice, for every [`Style`] from now on.
pub fn set_color(c: Option<bool>) {
    let _ = COLOR.set(c);
}

/// Colours on standard error: `--color`, else none under `NO_COLOR`,
/// else on a terminal or under `CLICOLOR_FORCE`.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
    /// Standard error is a terminal: symbols, and a spinner.
    pub tty: bool,
}

impl Style {
    pub fn detect() -> Style {
        Style::on(std::io::stderr().is_terminal())
    }

    /// For standard output (a command's answer: `config`, `news`).
    pub fn stdout() -> Style {
        Style::on(std::io::stdout().is_terminal())
    }

    /// For a stream that is a terminal (`tty`) or not: `--color`'s
    /// choice, else colours on a terminal, or forced by `CLICOLOR_FORCE`.
    fn on(tty: bool) -> Style {
        if let Some(&Some(color)) = COLOR.get() {
            return Style { color, tty };
        }
        // (no-color.org: NO_COLOR, set and not empty, wins over everything
        // but the command line's --color; CLICOLOR_FORCE colours a pipe)
        let set = |v: &str| std::env::var_os(v).is_some_and(|x| !x.is_empty() && x != "0");
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|x| !x.is_empty());
        Style {
            color: !no_color && (set("CLICOLOR_FORCE") || tty),
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

/// A build's reason as a reader reads it: the session's `cold` (its
/// first build, or one asked from nothing) is a clean build; a file's
/// name, an edit's place and the rest are as they are.
pub fn said_why(why: &str) -> String {
    why.split(", ")
        .map(|w| match w {
            "cold" => "clean build",
            "asked: a cold build" => "a clean build was asked",
            w => w,
        })
        .collect::<Vec<_>>()
        .join(", ")
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
pub fn clip(s: &str, max: usize) -> String {
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
    /// The watch's status line as given ([`Ui::status`]): its text, its
    /// mark and the style, laid out at the terminal's width each time it
    /// is drawn (a line laid out for a wider terminal wraps when it
    /// narrows, and its copies piled up under every line printed).
    idle: Option<(String, Option<bool>, Style)>,
    /// The keys shown at the status line's right end.
    keys: Option<&'static str>,
    /// The lines on the screen now, and each one's width when drawn.
    drawn: usize,
    drawn_widths: Vec<usize>,
    /// Nothing drawn below any more (the watch is quitting).
    gone: bool,
}

impl Bottom {
    fn shown(&self) -> Vec<String> {
        if !self.spin.is_empty() {
            return self.spin.clone();
        }
        let w = crate::keys::width().unwrap_or(80).saturating_sub(1);
        self.idle
            .iter()
            .map(|(t, ok, st)| idle_line(*st, t, *ok, self.keys.unwrap_or(""), w))
            .collect()
    }

    /// The lines on the screen erased, the cursor at the first's start:
    /// each counted by the rows it takes at the terminal's width now (a
    /// terminal that narrowed has wrapped the wider ones), then the
    /// screen cleared from the first down.
    fn erase(&mut self, e: &mut impl std::io::Write) {
        if self.drawn > 0 {
            let cols = crate::keys::width().unwrap_or(80).max(1);
            let rows: usize = self
                .drawn_widths
                .iter()
                .map(|&w| w.div_ceil(cols).max(1))
                .sum::<usize>()
                .max(self.drawn);
            let _ = write!(e, "\r");
            if rows > 1 {
                let _ = write!(e, "\x1b[{}A", rows - 1);
            }
            let _ = write!(e, "\x1b[J");
            self.drawn = 0;
            self.drawn_widths.clear();
        }
    }
}

/// The watch's status line at width `w`: its mark, its text, and the
/// keys at the right end when there is room.
fn idle_line(st: Style, t: &str, ok: Option<bool>, keys: &str, w: usize) -> String {
    let mark = match ok {
        Some(true) => st.green("●"),
        Some(false) => st.red("●"),
        None => st.dim("●"),
    };
    let room = w.saturating_sub(2);
    let fits = !keys.is_empty() && t.chars().count() + 3 + keys.chars().count() <= room;
    if fits {
        let gap = room - t.chars().count() - keys.chars().count();
        format!("{mark} {}{}{}", st.dim(t), " ".repeat(gap), st.dim(keys))
    } else {
        format!("{mark} {}", st.dim(&clip(t, room)))
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
    /// The pages of the build the spinner shows ([`Ui::streamed`]).
    pages: Arc<Mutex<PageLog>>,
}

/// A spinner while a build runs, until [`Spin::done`].
pub struct Spin {
    bottom: Arc<Mutex<Bottom>>,
    tap: Arc<Mutex<Tap>>,
    pages: Arc<Mutex<PageLog>>,
    stop: Arc<AtomicBool>,
    updater: Option<std::thread::JoinHandle<()>>,
}

/// The pages of the build the spinner shows, for its live tree
/// ([`PageLog::lines`]):
///
/// - A clean build's pages as they are typeset (the engine's highest page
///   reached, looked at each tick, [`PageLog::observe`]), `○` with its
///   time: it may still change, for PhiTeX settles the read-back files
///   (the contents, `\ref`, the index) inside the one evaluation, re-making
///   the pages whose read-back values changed.
/// - A page made again with another hash ([`PageLog::streamed`], the
///   pages the engine streams) is `↻ redone`.
/// - `✓` once the build is over ([`PageLog::end`]): every page is final
///   then, and only then. Marking each page `✓` while the build runs
///   needs the engine to say which pages nothing pending can reach any
///   more ("settled up to page N" on its progress), which is 1.0.1 (#18).
/// - An edit (the last build made pages) lists no page while it runs,
///   and at its end only the pages it changed (`↻ page 4`).
#[derive(Default)]
pub struct PageLog {
    /// The engine's evaluation, and its highest page, last seen, and when.
    eval: u64,
    page: u64,
    at: Duration,
    /// The pages the last build made (0: none, a clean build).
    before: usize,
    /// A clean build's pages, from 1.
    rows: Vec<PageRow>,
    /// The build is over: its pages, and those that differ from the last
    /// build's.
    end: Option<(usize, Vec<usize>)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PageRow {
    /// Its time (none: streamed before it was seen typeset).
    took: Option<Duration>,
    /// The hash it was streamed with last.
    hash: Option<u64>,
    /// Streamed again with another hash.
    redone: bool,
}

/// The pages a line each, the latest; the earlier ones folded.
const SHOWN: usize = 6;

/// `page 4`, `pages 1–20`, `pages 2, 5–6`.
fn pages_said(ks: &[usize]) -> String {
    match ks {
        [k] => format!("page {k}"),
        _ => format!("pages {}", crate::view::ranges(ks)),
    }
}

/// [`pages_said`], or their count when they are many runs apart
/// (`160 pages`): a line, not a list.
fn pages_brief(ks: &[usize]) -> String {
    let runs = 1 + ks.windows(2).filter(|w| w[1] != w[0] + 1).count();
    if runs <= 4 {
        pages_said(ks)
    } else {
        format!("{} pages", ks.len())
    }
}

/// Where the build is: page `n` of the last build's `m` (0: none), never
/// past it (the engine's highest page may count pages a later run takes
/// back: a 32-page document's "page 33").
fn at_page(n: u64, m: usize) -> String {
    let m = u64::try_from(m).unwrap_or(u64::MAX);
    if m == 0 {
        format!("page {n}")
    } else {
        format!("page {} of {m}", n.min(m))
    }
}

impl PageLog {
    /// The engine at evaluation `eval`, its highest page `page`, at `now`
    /// into the build, the last build's pages `before`: the pages reached
    /// since the last look added, the time between the looks shared by
    /// them.
    pub fn observe(&mut self, eval: u64, page: u64, before: usize, now: Duration) {
        if eval == 0 || self.end.is_some() {
            return;
        }
        self.before = before;
        if eval != self.eval {
            // (a new evaluation: its pages counted from none)
            self.eval = eval;
            self.page = 0;
        }
        if page <= self.page {
            if page == 0 {
                self.at = now;
            }
            return;
        }
        let n = u32::try_from(page - self.page).unwrap_or(u32::MAX);
        let took = now.saturating_sub(self.at) / n.max(1);
        if before == 0 {
            for k in self.page + 1..=page {
                self.row(k).took = Some(took);
            }
        }
        self.page = page;
        self.at = now;
    }

    /// Page `k`'s row (from 1), made with those before it.
    fn row(&mut self, k: u64) -> &mut PageRow {
        let k = usize::try_from(k).unwrap_or(usize::MAX).max(1);
        if self.rows.len() < k {
            self.rows.resize(k, PageRow::default());
        }
        &mut self.rows[k - 1]
    }

    /// Page `page` streamed with hash `hash`: redone when it was streamed
    /// before with another.
    pub fn streamed(&mut self, page: u64, hash: u64) {
        if page == 0 || self.end.is_some() {
            return;
        }
        let r = self.row(page);
        if r.hash.is_some_and(|h| h != hash) {
            r.redone = true;
        }
        r.hash = Some(hash);
    }

    /// The build is over: its `pages`, final, and `changed` those that
    /// differ from the last build's (an edit's).
    pub fn end(&mut self, pages: usize, changed: &[usize]) {
        self.rows.truncate(pages);
        self.end = Some((pages, changed.to_vec()));
    }

    /// The tree's header: where the build is, or (over) its pages.
    pub fn header(&self, page: u64) -> String {
        match &self.end {
            Some((1, _)) => "1 page".to_owned(),
            Some((n, _)) => format!("{n} pages"),
            None => at_page(page, self.before),
        }
    }

    /// The tree's page lines, a mark and a text each.
    pub fn lines(&self, st: Style) -> Vec<(String, String)> {
        let redone: Vec<usize> = (1..=self.rows.len())
            .filter(|&k| self.rows[k - 1].redone)
            .collect();
        if let Some((n, changed)) = &self.end {
            // (an edit: the pages it changed, none if none did)
            if self.before > 0 {
                return if changed.is_empty() {
                    vec![(st.dim("·"), st.dim("no page changed"))]
                } else {
                    vec![(st.cyan("↻"), pages_brief(changed))]
                };
            }
            if *n == 0 {
                return Vec::new();
            }
            let all: Vec<usize> = (1..=*n).collect();
            let mut text = pages_said(&all);
            if !redone.is_empty() {
                text.push_str(&st.dim(&format!(" · {} redone", pages_brief(&redone))));
            }
            return vec![(st.green("✓"), text)];
        }
        if self.before > 0 {
            return Vec::new();
        }
        let n = self.rows.len();
        let cut = n.saturating_sub(SHOWN);
        let mut out = Vec::new();
        if cut > 0 {
            let folded: Vec<usize> = (1..=cut).collect();
            out.push((
                st.yellow("○"),
                format!("{} {}", pages_said(&folded), st.dim("· may change")),
            ));
            let again: Vec<usize> = redone.iter().copied().filter(|&k| k <= cut).collect();
            if !again.is_empty() {
                out.push((
                    st.cyan("↻"),
                    format!("{} {}", pages_brief(&again), st.dim("redone")),
                ));
            }
        }
        for k in cut + 1..=n {
            let r = self.rows[k - 1];
            let took = r.took.map_or_else(String::new, ms);
            let (mark, what) = if r.redone {
                (st.cyan("↻"), "redone")
            } else {
                (st.yellow("○"), "may change")
            };
            out.push((
                mark,
                format!(
                    "{} {}",
                    pad(&format!("page {k}"), 9, false),
                    st.dim(&format!("{} · {what}", pad(&took, 9, true)))
                ),
            ));
        }
        out
    }
}

/// `lines` under the tree's header, `├─` each, the last `└─`.
fn branches(st: Style, out: &mut Vec<String>, lines: &[String]) {
    for (k, l) in lines.iter().enumerate() {
        let branch = if k + 1 == lines.len() {
            "└─"
        } else {
            "├─"
        };
        out.push(format!("       {} {l}", st.dim(branch)));
    }
}

/// The live tree under the spinner's line, once a build has run a
/// while: each phase ended with its time, the one running with its time
/// so far, the cores busy and the memory held.
fn tree(
    st: Style,
    s: &Status,
    tick: char,
    load: Option<(f64, u64)>,
    w: usize,
    pages: &PageLog,
) -> Vec<String> {
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
            // (a node not in the main text: a piece of an included file,
            // or a page's output routine)
            let place = if op == 3 {
                "the output routine".to_owned()
            } else if at == phitex_tex::Progress::INSIDE {
                "an included file".to_owned()
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
        // (where the build is, as a reader counts: the page reached, of
        // the last build's pages; the runs only as a sign of life)
        let _ = (done, total);
        // (idle: the threads less those running; the slots are one more
        // than the threads, the evaluating thread's own)
        let _ = idle;
        let running = rows.iter().map(|r| r.1).sum::<usize>();
        let idle = live.threads.saturating_sub(running);
        let page = p.page();
        let at = match page {
            // (no piece has started: the format and the preamble load,
            // which are not counted as the document's runs)
            0 if p.format() == Some(false) => {
                "making the format (once per PhiTeX update)".to_owned()
            }
            0 if p.format().is_none() => "loading the format".to_owned(),
            0 if total == 0 => "loading the preamble".to_owned(),
            0 => "before the first page".to_owned(),
            n => pages.header(n),
        };
        if total == 0 {
            out.push(format!("       {}", st.bold(&at)));
        } else {
            out.push(format!(
                "       {} {}",
                st.bold(&at),
                st.dim(&format!("· {running} running · {idle} idle"))
            ));
        }
        // (the pages typeset, then what the workers run now)
        let page_lines = pages.lines(st);
        let shown = rows.len().min(3);
        let rows: Vec<String> = rows
            .iter()
            .take(shown)
            .map(|(row, n)| {
                if *n > 1 {
                    format!("{row}{}", st.dim(&format!(" ×{n}")))
                } else {
                    row.clone()
                }
            })
            .collect();
        let lines: Vec<String> = page_lines
            .into_iter()
            .map(|(mark, text)| format!("{mark} {text}"))
            .chain(rows)
            .collect();
        branches(st, &mut out, &lines);
    } else {
        // (the engine done, the build over: its pages, final, while the
        // files are written)
        let lines: Vec<String> = pages
            .lines(st)
            .into_iter()
            .map(|(mark, text)| format!("{mark} {text}"))
            .collect();
        if !lines.is_empty() {
            out.push(format!("       {}", st.bold(&pages.header(0))));
            branches(st, &mut out, &lines);
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
/// The question `q`, the bottom lines erased first, with a blank after it
/// and no line's end: the key typed next is answered after it, on its line.
fn asked(e: &mut impl std::io::Write, b: &mut Bottom, q: &str) {
    b.erase(e);
    let _ = write!(e, "{q} ");
    let _ = e.flush();
}

fn redraw(e: &mut impl std::io::Write, b: &mut Bottom) {
    b.erase(e);
    if b.gone {
        let _ = e.flush();
        return;
    }
    let lines = b.shown();
    let _ = write!(e, "{}", lines.join("\n"));
    b.drawn = lines.len();
    b.drawn_widths = lines.iter().map(|l| width(l)).collect();
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
            pages: Arc::default(),
        }
    }

    /// Page `page` of the build the spinner shows was streamed with hash
    /// `hash` (the engine's stream hook, on its thread): shown redone
    /// when it was streamed before with another.
    pub fn streamed(&self, page: u64, hash: u64) {
        lock(&self.pages).streamed(page, hash);
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

    /// A question on standard error, left open after `q` and a blank for
    /// its answer (the cursor where a key is typed), until
    /// [`Ui::answered`].
    pub fn ask(&self, q: &str) {
        if !self.json {
            let mut b = lock(&self.bottom);
            asked(&mut std::io::stderr().lock(), &mut b, q);
        }
        self.tell("line", q);
    }

    /// The asked question's answer, `a` (none: no key came), and its line
    /// ended: what is printed next starts a line of its own.
    pub fn answered(&self, a: Option<&str>) {
        if !self.json {
            let mut e = std::io::stderr().lock();
            let _ = writeln!(e, "{}", a.unwrap_or(""));
            let _ = e.flush();
        }
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
            b.idle = text.map(|t| (t.to_owned(), ok, st));
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

    /// `keys` at the status line's right end from now on; and, the watch
    /// being on a terminal, the bottom lines drawn again when the
    /// terminal's width changes (looked at four times a second: a resize
    /// while idle left the status line wrapped until the next build).
    pub fn keys(&self, keys: &'static str) {
        static WATCHING: AtomicBool = AtomicBool::new(false);
        lock(&self.bottom).keys = Some(keys);
        if self.st.tty && !self.json && !WATCHING.swap(true, Ordering::SeqCst) {
            let bottom = self.bottom.clone();
            let _ = std::thread::Builder::new()
                .name("phitex-resize".into())
                .spawn(move || {
                    let mut was = crate::keys::width();
                    loop {
                        std::thread::sleep(Duration::from_millis(250));
                        let now = crate::keys::width();
                        // (back from Ctrl-Z: the shell's lines are over
                        // the bottom ones, drawn again below them)
                        if crate::keys::resumed() {
                            let mut b = lock(&bottom);
                            if b.gone {
                                return;
                            }
                            b.drawn = 0;
                            b.drawn_widths.clear();
                            redraw(&mut std::io::stderr().lock(), &mut b);
                            was = now;
                            continue;
                        }
                        if now == was {
                            continue;
                        }
                        was = now;
                        let mut b = lock(&bottom);
                        if b.gone {
                            return;
                        }
                        if b.drawn > 0 {
                            redraw(&mut std::io::stderr().lock(), &mut b);
                        }
                    }
                });
        }
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
        *lock(&self.pages) = PageLog::default();
        let pages = self.pages.clone();
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
                if let Some(live) = status.live() {
                    let p = &live.progress;
                    lock(&pages).observe(p.evaluation(), p.page(), live.pages_before, t0.elapsed());
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
                        lines.extend(tree(st, &status, tick, load.now, w, &lock(&pages)));
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
            pages: self.pages.clone(),
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
    /// The build is over (the engine returned; the files may still be
    /// written): its `pages` all final, `changed` those that differ from
    /// the last build's, shown at once.
    pub fn built(&self, pages: usize, changed: &[usize]) {
        lock(&self.pages).end(pages, changed);
        if let Some(u) = &self.updater {
            u.thread().unpark();
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The quit's question leaves the cursor after "[y/N] ", not on the
    /// line below (it was printed as a line); the answer then ends it.
    #[test]
    fn a_question_is_left_open_for_its_key() {
        let mut b = Bottom::default();
        let mut out = Vec::new();
        asked(
            &mut out,
            &mut b,
            "write x.pdf and its .synctex.gz here? [y/N]",
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "write x.pdf and its .synctex.gz here? [y/N] "
        );
    }

    fn plain_style() -> Style {
        Style {
            color: false,
            tty: false,
        }
    }

    /// The text of `l`'s lines, each mark and text as one.
    fn said(l: &PageLog) -> Vec<String> {
        l.lines(plain_style())
            .into_iter()
            .map(|(m, t)| format!("{m} {t}"))
            .collect()
    }

    /// A clean build's pages as they are typeset, each with its time, may
    /// change; the earlier ones folded, the last few a line each.
    #[test]
    fn pages_typeset_fold_the_earlier_ones() {
        let ms = Duration::from_millis;
        let mut l = PageLog::default();
        l.observe(7, 0, 0, ms(0));
        l.observe(7, 2, 0, ms(400));
        l.observe(7, 3, 0, ms(700));
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.rows[0].took, Some(ms(200)));
        assert_eq!(l.rows[2].took, Some(ms(300)));
        assert_eq!(
            said(&l),
            [
                "○ page 1     200.0 ms · may change",
                "○ page 2     200.0 ms · may change",
                "○ page 3     300.0 ms · may change",
            ]
        );
        l.observe(7, 30, 0, ms(3400));
        let s = said(&l);
        assert_eq!(s.len(), 1 + SHOWN, "{s:?}");
        assert_eq!(s[0], "○ pages 1–24 · may change");
        assert!(s[1].starts_with("○ page 25 "), "{s:?}");
        assert!(s[SHOWN].starts_with("○ page 30 "), "{s:?}");
        assert_eq!(l.header(30), "page 30");
    }

    /// When the build is over every page is final: ✓, folded, the pages
    /// past its last (counted by a run taken back) gone.
    #[test]
    fn every_page_final_when_the_build_ends() {
        let ms = Duration::from_millis;
        let mut l = PageLog::default();
        l.observe(1, 0, 0, ms(0));
        l.observe(1, 33, 0, ms(3300));
        l.end(32, &[]);
        assert_eq!(said(&l), ["✓ pages 1–32"]);
        assert_eq!(l.header(0), "32 pages");
        assert_eq!(l.rows.len(), 32);
        // (nothing seen after the end changes it)
        l.observe(1, 40, 0, ms(4000));
        l.streamed(3, 9);
        assert_eq!(said(&l), ["✓ pages 1–32"]);
        let mut one = PageLog::default();
        one.observe(1, 1, 0, ms(10));
        one.end(1, &[]);
        assert_eq!(said(&one), ["✓ page 1"]);
        assert_eq!(one.header(0), "1 page");
    }

    /// A page streamed again with another hash is redone (the contents
    /// re-made once the headings are known); the same hash again (the
    /// page found exact) is not.
    #[test]
    fn a_page_streamed_again_with_another_hash_is_redone() {
        let ms = Duration::from_millis;
        let mut l = PageLog::default();
        l.observe(1, 0, 0, ms(0));
        l.streamed(1, 11);
        l.streamed(2, 22);
        l.observe(1, 2, 0, ms(200));
        l.streamed(2, 22);
        assert!(!l.rows[1].redone);
        l.streamed(2, 23);
        l.observe(1, 3, 0, ms(300));
        let s = said(&l);
        assert!(
            s[1].starts_with("↻ page 2 ") && s[1].ends_with("· redone"),
            "{s:?}"
        );
        assert!(s[0].ends_with("· may change"), "{s:?}");
        // (folded: its own line under the fold)
        l.observe(1, 20, 0, ms(2000));
        let s = said(&l);
        assert_eq!(s[0], "○ pages 1–14 · may change");
        assert_eq!(s[1], "↻ page 2 redone");
        assert_eq!(s.len(), 2 + SHOWN, "{s:?}");
        // (at the end: final, and said to have been redone)
        l.end(20, &[]);
        assert_eq!(said(&l), ["✓ pages 1–20 · page 2 redone"]);
        // (many, far apart: counted, a line not a list)
        let mut l = PageLog::default();
        l.observe(1, 40, 0, ms(4000));
        for k in [2, 5, 9, 13, 17, 30] {
            l.streamed(k, 1);
            l.streamed(k, 2);
        }
        assert_eq!(said(&l)[1], "↻ 6 pages redone");
        l.end(40, &[]);
        assert_eq!(said(&l), ["✓ pages 1–40 · 6 pages redone"]);
    }

    /// The page the build is at never goes past the last build's last
    /// page: a 32-page document's "page 33" is page 32 of 32.
    #[test]
    fn the_page_reached_is_bounded_by_the_last() {
        assert_eq!(at_page(33, 32), "page 32 of 32");
        assert_eq!(at_page(5, 32), "page 5 of 32");
        assert_eq!(at_page(33, 0), "page 33");
        let mut l = PageLog::default();
        l.observe(4, 33, 32, Duration::from_millis(50));
        assert_eq!(l.header(33), "page 32 of 32");
    }

    /// An edit lists no page while it runs, and at its end only the pages
    /// it changed (or that none did), never the whole document.
    #[test]
    fn an_edit_lists_only_the_pages_it_changed() {
        let ms = Duration::from_millis;
        let mut l = PageLog::default();
        l.observe(9, 0, 32, ms(0));
        l.observe(9, 32, 32, ms(400));
        assert_eq!(said(&l), Vec::<String>::new());
        assert_eq!(l.rows, []);
        l.end(32, &[4]);
        assert_eq!(said(&l), ["↻ page 4"]);
        let mut l = PageLog::default();
        l.observe(9, 10, 32, ms(400));
        l.end(32, &[4, 5, 9]);
        assert_eq!(said(&l), ["↻ pages 4–5, 9"]);
        let mut l = PageLog::default();
        l.observe(9, 10, 32, ms(400));
        l.end(32, &[1, 3, 5, 7, 9]);
        assert_eq!(said(&l), ["↻ 5 pages"]);
        let mut l = PageLog::default();
        l.observe(9, 10, 32, ms(400));
        l.end(32, &[]);
        assert_eq!(said(&l), ["· no page changed"]);
    }

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

    /// A status line drawn 150 columns wide, the terminal then 80 (no
    /// terminal in a test: 80): it takes two rows now, and the erase goes
    /// up one and clears to the screen's end, not one row only (the copy
    /// left under every printed line).
    #[test]
    fn erase_counts_the_rows_a_wider_line_wraps_to() {
        let mut b = Bottom {
            drawn: 1,
            drawn_widths: vec![150],
            ..Bottom::default()
        };
        let mut out = Vec::new();
        b.erase(&mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[1A\x1b[J");
        assert_eq!(b.drawn, 0);
        let st = Style {
            color: false,
            tty: false,
        };
        let l = idle_line(
            st,
            "amsldoc.tex · build 1 · 44 pages",
            Some(true),
            "h: keys",
            30,
        );
        assert!(width(&l) <= 30, "{l}");
    }
}
