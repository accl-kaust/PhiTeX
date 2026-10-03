//! The modern command line's terminal (DESIGN.md 2.5): status lines like
//! cargo's, a live line that moves while a build runs, errors as
//! rustc-style snippets, warnings grouped at the end, and in `partex
//! watch` one line per rebuild above a footer. Everything goes to
//! standard error; the TeX transcript still goes, whole, to the `.log`
//! file.
//!
//! On a terminal the live line is drawn by a thread of its own, about
//! twelve times a second, from what the build tells it (its passes and
//! phases) and from the engine's progress board
//! (`partex_core::progress`: the commands run, the pages shipped, the
//! file and line being read), so it moves while the engine runs: the
//! build's thread never draws it. Elsewhere (a pipe, a file, CI) the
//! same lines come out plain, as they happen, and nothing is redrawn.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use partex_core::progress::{BOARD, Snapshot};

use crate::events::{Phase, Progress};
use crate::live::{Live, Task};
use crate::warnings::{Group, Summary};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Colours, symbols and hyperlinks, or none.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
    /// Symbols beyond ASCII.
    pub unicode: bool,
    /// OSC 8 hyperlinks on file names.
    pub links: bool,
}

impl Style {
    /// Text only (with Unicode symbols).
    #[cfg(test)]
    #[must_use]
    pub const fn plain() -> Self {
        Self {
            color: false,
            unicode: true,
            links: false,
        }
    }

    fn paint(self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_owned()
        }
    }
    pub fn green(self, s: &str) -> String {
        self.paint("1;32", s)
    }
    pub fn red(self, s: &str) -> String {
        self.paint("1;31", s)
    }
    pub fn yellow(self, s: &str) -> String {
        self.paint("1;33", s)
    }
    pub fn blue(self, s: &str) -> String {
        self.paint("1;34", s)
    }
    pub fn cyan(self, s: &str) -> String {
        self.paint("1;36", s)
    }
    pub fn bold(self, s: &str) -> String {
        self.paint("1", s)
    }
    pub fn dim(self, s: &str) -> String {
        self.paint("2", s)
    }

    /// `fancy` with Unicode, else `ascii`.
    #[must_use]
    pub fn sym(self, fancy: &'static str, ascii: &'static str) -> &'static str {
        if self.unicode { fancy } else { ascii }
    }

    /// What separates the facts of a line: ` · `.
    #[must_use]
    pub fn sep(self) -> &'static str {
        self.sym(" · ", ", ")
    }

    /// `text`, a link to the file at `path` where the terminal has them.
    #[must_use]
    pub fn link(self, text: &str, path: &str) -> String {
        if !self.links {
            return text.to_owned();
        }
        let Ok(abs) = std::path::absolute(path) else {
            return text.to_owned();
        };
        let mut url = format!("file://{}", hostname());
        for &b in abs.as_os_str().as_encoded_bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                url.push(char::from(b));
            } else {
                let _ = write!(url, "%{b:02X}");
            }
        }
        format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
    }
}

/// This machine's name, for `file://` links.
fn hostname() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|h| h.trim().to_owned())
            .unwrap_or_default()
    })
}

/// How much to show.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub style: Style,
    /// Draw a live line (a terminal).
    pub progress: bool,
    /// 0: status and problems; 1: also what the document shows on the
    /// terminal (`\message`, `\typeout`); 2: also TeX's raw terminal
    /// stream.
    pub verbose: u8,
    /// Only problems and the final line.
    pub quiet: bool,
}

/// A build's size, as the last full build of the same job measured it:
/// how far a build from the start is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Estimate {
    /// Commands run (on the progress board).
    pub commands: u64,
    /// Pages shipped out.
    pub pages: u64,
    pub millis: u64,
}

impl Estimate {
    /// The text form kept in the cache.
    #[must_use]
    pub fn to_text(self) -> String {
        format!(
            "commands {}\npages {}\nmillis {}\n",
            self.commands, self.pages, self.millis
        )
    }

    /// An estimate's text form read back.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut e = Self::default();
        for line in text.lines() {
            let (k, v) = line.split_once(' ')?;
            let v: u64 = v.trim().parse().ok()?;
            match k {
                "commands" => e.commands = v,
                "pages" => e.pages = v,
                "millis" => e.millis = v,
                _ => {}
            }
        }
        (e.commands > 0).then_some(e)
    }
}

/// `n` things.
#[must_use]
pub fn plural(n: usize, one: &str, many: &str) -> String {
    let count = thousands(u64::try_from(n).unwrap_or(u64::MAX));
    format!("{count} {}", if n == 1 { one } else { many })
}

/// `n` with its thousands separated: `98,803`.
#[must_use]
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `n` in three figures: `812`, `12.3k`, `1.24M`.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn compact(n: u64) -> String {
    let f = n as f64;
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", f / 1e3).replace(".0k", "k")
    } else {
        format!("{:.2}M", f / 1e6)
    }
}

/// A file's size: `812 B`, `84 KB`, `1.2 MB`.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn size(bytes: usize) -> String {
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < 10.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    }
}

/// A duration as people read it: `38 ms`, `1.23 s`, `12.3 s`, `2 min
/// 05 s`.
#[must_use]
pub fn secs(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{:.0} ms", s * 1e3)
    } else if s < 10.0 {
        format!("{s:.2} s")
    } else if s < 60.0 {
        format!("{s:.1} s")
    } else {
        let whole = d.as_secs();
        format!("{} min {:02} s", whole / 60, whole % 60)
    }
}

/// The local time of day: `17:03:12`.
fn clock_time(t: SystemTime) -> String {
    static ZONE: std::sync::OnceLock<crate::clock::Zone> = std::sync::OnceLock::new();
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let local = secs + i64::from(ZONE.get_or_init(crate::clock::Zone::local).offset(secs));
    let day = local.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, day / 60 % 60, day % 60)
}

/// What a finished build shows.
pub struct End<'a> {
    pub file: &'a str,
    pub summary: &'a Summary,
    /// TeX's terminal stream (`-vv`).
    pub term: &'a [u8],
    /// The main output file (PDF or DVI), and its size.
    pub output: Option<String>,
    pub bytes: Option<usize>,
    pub failed: bool,
    pub checked: bool,
    /// A rebuild of `partex watch`: one line in its log.
    pub rebuild: Option<Rebuild>,
}

/// What a rebuild of `partex watch` was for.
pub struct Rebuild {
    /// What changed: `paper.tex:18`.
    pub changed: Vec<String>,
}

/// How a problem is known from one build to the next (its line moves
/// with edits above it).
fn identities(summary: &Summary, s: Style, verbose: u8) -> (Vec<String>, Vec<String>) {
    let errors = summary
        .errors
        .iter()
        .map(|d| crate::snippet::error(d, s, verbose))
        .collect();
    let mut warnings = Vec::new();
    for g in &summary.groups {
        for i in &g.items {
            let what = if i.excerpt.is_empty() {
                i.subject.clone()
            } else {
                format!("{} {}", i.excerpt, i.amount)
            };
            warnings.push(format!("{} {what}", g.kind.code()));
        }
    }
    (errors, warnings)
}

/// The bookkeeping of the build being rendered.
struct Run {
    started: Instant,
    passes: usize,
    /// The board when the pass began.
    pass_base: Snapshot,
    /// A watch's rebuild (its passes are said only with `-v`).
    rebuild: bool,
    /// The last build's totals, if they apply (a build from the start).
    estimate: Option<Estimate>,
    /// Pass 1 of this build ran from the start.
    cold: bool,
    /// This build's totals, if it ran from the start.
    measured: Option<Estimate>,
    /// The commands its passes ran, and the job's in all (the last's).
    commands: u64,
    total: u64,
    /// The problems of the last build shown (a watch shows what is new),
    /// and its counts.
    last: Option<(Vec<String>, Vec<String>)>,
    was: Was,
    /// The last full summary (`w` shows it again).
    summary: Option<Summary>,
}

/// The renderer of one command.
pub struct Renderer {
    pub settings: Settings,
    live: Arc<Live>,
    run: Mutex<Run>,
    /// Whether the watch's plain `Watching` line was printed.
    said_watching: std::sync::atomic::AtomicBool,
}

impl Renderer {
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            live: Live::new(settings),
            run: Mutex::new(Run {
                started: Instant::now(),
                passes: 0,
                pass_base: Snapshot::default(),
                rebuild: false,
                estimate: None,
                cold: false,
                measured: None,
                commands: 0,
                total: 0,
                last: None,
                was: Was::default(),
                summary: None,
            }),
            said_watching: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn style(&self) -> Style {
        self.settings.style
    }

    /// Whether `-v` was given.
    #[must_use]
    pub fn verbose(&self) -> bool {
        self.settings.verbose > 0
    }

    /// The live area, for another thread (the watch's keys).
    #[must_use]
    pub fn live(&self) -> Arc<Live> {
        self.live.clone()
    }

    /// A status line: `   Compiling paper.tex (pdflatex)`.
    pub fn status(&self, verb: &str, what: &str) {
        if self.settings.quiet {
            return;
        }
        self.line(&format!(
            "{} {what}",
            self.style().green(&format!("{verb:>12}"))
        ));
    }

    /// A dim line under the status lines (a hint, an answer to a key).
    pub fn note(&self, text: &str) {
        self.live.note(text);
    }

    /// A line on standard error, above the live line.
    pub fn line(&self, text: &str) {
        self.live.print(text);
    }

    /// A build begins.
    pub fn start(&self) {
        let mut r = lock(&self.run);
        r.started = Instant::now();
        r.passes = 0;
        r.rebuild = false;
        r.cold = false;
        r.measured = None;
        r.commands = 0;
        r.total = 0;
    }

    /// A rebuild of `partex watch` begins.
    pub fn start_rebuild(&self) {
        self.start();
        lock(&self.run).rebuild = true;
    }

    /// The totals of the last build of the job from its start: a build
    /// from the start shows how far it is.
    pub fn set_estimate(&self, e: Option<Estimate>) {
        lock(&self.run).estimate = e;
    }

    /// This build's totals, if it ran from the start (for the next).
    #[must_use]
    pub fn measured(&self) -> Option<Estimate> {
        lock(&self.run).measured
    }

    /// Show `verb` and `what` on the live line until the next task, the
    /// end of the build or [`Renderer::idle`]: the engine's progress too
    /// if `engine`.
    pub fn task(&self, verb: &str, what: &str, engine: bool) {
        self.live
            .begin(Task::new(verb, what, engine, Instant::now()));
    }

    /// Nothing runs any more: the live line goes (the footer, if any,
    /// comes back).
    pub fn idle(&self) {
        self.live.end();
    }

    /// The live area cleared, for good (before the process exits).
    pub fn close(&self) {
        self.live.close();
    }

    /// What a converging build did.
    pub fn progress(&self, p: &Progress) {
        match p {
            Progress::PassStart(n) => {
                partex_core::progress::BOARD.phase(partex_core::progress::Phase::Run);
                let (rebuild, started) = {
                    let mut r = lock(&self.run);
                    r.pass_base = BOARD.snapshot();
                    (r.rebuild, r.started)
                };
                let verb = if rebuild && *n == 1 {
                    String::from("Rebuilding")
                } else {
                    format!("Pass {n}")
                };
                self.live.begin(Task::new(verb, "", true, started));
            }
            Progress::Phase(phase) => self.phase(*phase),
            Progress::Pass(n, report) => self.pass_ended(*n, *report),
            Progress::Tool(line) => {
                let line = line.strip_prefix("partex: ").unwrap_or(line);
                let (tool, rest) = line.split_once(' ').unwrap_or((line, ""));
                if !self.settings.quiet {
                    self.line(&format!(
                        "{} {rest}",
                        self.style().blue(&format!("{tool:>12}"))
                    ));
                }
            }
        }
    }

    fn phase(&self, phase: Phase) {
        let (verb, what) = match phase {
            Phase::Cold => {
                let mut r = lock(&self.run);
                r.cold = true;
                let e = r.estimate;
                drop(r);
                self.live.estimate(e);
                return;
            }
            Phase::Loading => ("Loading", "the saved build"),
            Phase::Linking => ("Linking", "the outputs"),
            Phase::Writing => ("Writing", "the outputs"),
            Phase::Saving => ("Saving", "the build for the next run"),
        };
        // (a load or a save on its own, else the build's time)
        let started = match phase {
            Phase::Loading | Phase::Saving => Instant::now(),
            _ => lock(&self.run).started,
        };
        self.live.begin(Task::new(verb, what, false, started));
    }

    /// Pass `n` ended, having done `report` (`None`: nothing changed).
    fn pass_ended(&self, n: usize, report: Option<&crate::session::Report>) {
        let s = self.style();
        let board = BOARD.snapshot();
        let (base, rebuild, cold) = {
            let r = lock(&self.run);
            (r.pass_base.clone(), r.rebuild, r.cold)
        };
        let pages = board.pages.saturating_sub(base.pages);
        let Some(r) = report else {
            if n == 1 {
                self.status("Fresh", "nothing changed since the last build");
            }
            return;
        };
        {
            let mut run = lock(&self.run);
            run.passes = n.max(1);
            run.commands += r.commands;
            run.total = r.total_commands;
        }
        if n == 1 && cold {
            lock(&self.run).measured = Some(Estimate {
                commands: board.commands.saturating_sub(base.commands),
                pages,
                millis: u64::try_from(r.elapsed.as_millis()).unwrap_or(u64::MAX),
            });
        }
        if self.settings.quiet || (rebuild && self.settings.verbose == 0) {
            return;
        }
        let sep = s.sep();
        let mut what = Vec::new();
        if r.commands < r.total_commands {
            what.push(format!(
                "{} of {} commands run again",
                thousands(r.commands),
                thousands(r.total_commands)
            ));
        } else {
            what.push(format!("{} commands", thousands(r.total_commands)));
        }
        if let Some(c) = r.cut_at {
            what.push(format!("converged early at command {}", thousands(c)));
        }
        if pages > 0 {
            what.push(plural(usize::try_from(pages).unwrap_or(0), "page", "pages"));
        }
        what.push(secs(r.elapsed));
        self.line(&format!(
            "{} {}",
            s.cyan(&format!("{:>12}", format!("Pass {n}"))),
            what.join(sep)
        ));
        if self.settings.verbose > 0 {
            for w in &r.why {
                self.note(w);
            }
        }
    }

    /// `partex watch` watches `file` now: its footer (on a terminal; the
    /// keys it reads if `keys`), or once a plain line.
    pub fn watching(&self, file: &str, keys: bool) {
        if !self.live.footer(file, keys)
            && !self
                .said_watching
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.status("Watching", &format!("{file} (q and Enter to quit)"));
        }
    }

    /// Print the last build's problems again, in full (`w`).
    pub fn show_problems(&self) {
        let summary = lock(&self.run).summary.take();
        let Some(summary) = summary else {
            self.note("no build yet");
            return;
        };
        let text = self.problems(&summary, None);
        if text.is_empty() {
            self.note("no errors and no warnings");
        } else {
            self.line(text.trim_end());
        }
        lock(&self.run).summary = Some(summary);
    }

    /// Errors and warnings of `summary` as the terminal shows them; with
    /// `last` (a watch's rebuild), only what is new since, and a count of
    /// what went.
    fn problems(&self, summary: &Summary, last: Option<&(Vec<String>, Vec<String>)>) -> String {
        let s = self.style();
        let mut out = String::new();
        let (errors, warnings) = identities(summary, s, self.settings.verbose);
        // (the same error at the same place, once, with a count)
        let mut shown: Vec<(&String, usize)> = Vec::new();
        for text in &errors {
            match shown.iter_mut().find(|(t, _)| *t == text) {
                Some((_, n)) => *n += 1,
                None => shown.push((text, 1)),
            }
        }
        for (text, n) in shown {
            if let Some((old, _)) = last
                && old.contains(text)
            {
                // (as before: its headline)
                let head = text.lines().next().unwrap_or_default();
                let at = crate::term::strip(text.lines().nth(1).unwrap_or_default());
                let at = at.trim().trim_start_matches("-->").trim();
                let _ = writeln!(out, "{head} {}", s.dim(&format!("(as before, at {at})")));
                continue;
            }
            out.push_str(text);
            if n > 1 {
                let _ = writeln!(out, "{} {n} times", s.blue("   ="));
            }
            out.push('\n');
        }
        match last {
            None => {
                for g in &summary.groups {
                    let _ = writeln!(out, "{}", group(g, s));
                }
            }
            Some((_, old)) => {
                // (the warnings that are new, as groups)
                let mut k = 0;
                for g in &summary.groups {
                    let mut new = Group {
                        kind: g.kind,
                        items: Vec::new(),
                    };
                    for i in &g.items {
                        if !old.contains(&warnings[k]) {
                            new.items.push(i.clone());
                        }
                        k += 1;
                    }
                    if !new.items.is_empty() {
                        let _ = writeln!(out, "{}", group(&new, s).replacen(": ", ": new: ", 1));
                    }
                }
                let gone = old.iter().filter(|w| !warnings.contains(w)).count();
                if gone > 0 {
                    let _ = writeln!(
                        out,
                        "{:>12} {}",
                        "",
                        s.dim(&format!("{} gone", plural(gone, "warning", "warnings")))
                    );
                }
            }
        }
        out
    }

    /// The end of a build: problems, then one line on the result.
    pub fn finish(&self, end: &End) {
        let s = self.style();
        self.live.end_now();
        let mut out = String::new();
        if self.settings.verbose >= 2 && !end.term.is_empty() {
            out.push_str(&String::from_utf8_lossy(end.term));
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        if self.settings.verbose >= 1 {
            for m in &end.summary.messages {
                let _ = writeln!(out, "{} {m}", s.dim("message:"));
            }
        }
        let (totals, last, was) = {
            let r = lock(&self.run);
            let t = Totals {
                elapsed: r.started.elapsed(),
                passes: r.passes,
                ran: r.commands.min(r.total),
                total: r.total,
            };
            (t, r.last.clone(), r.was)
        };
        if let Some(rb) = &end.rebuild {
            // (a watch's log: the line first, then what is new)
            out.push_str(&rebuild_line(end, rb, &totals, was, s));
            out.push_str(&self.problems(end.summary, last.as_ref()));
        } else {
            out.push_str(&self.problems(end.summary, None));
            out.push_str(&result_line(end, &totals, s));
            if end.summary.warnings() > 0 && !self.settings.quiet {
                let _ = writeln!(
                    out,
                    "{:>12} {}",
                    "",
                    s.dim("every warning, and why the build ran as it did: `partex why`")
                );
            }
        }
        let elapsed = totals.elapsed;
        {
            let mut r = lock(&self.run);
            r.last = Some(identities(end.summary, s, self.settings.verbose));
            r.was = Was {
                pages: Some(end.summary.pages),
                warnings: Some(end.summary.warnings()),
            };
            r.summary = Some(Summary {
                errors: end.summary.errors.clone(),
                groups: end.summary.groups.clone(),
                messages: Vec::new(),
                pages: end.summary.pages,
            });
        }
        self.live.push_time(elapsed);
        self.line(out.trim_end_matches('\n'));
    }
}

/// What a build did in all, as its last line says.
struct Totals {
    elapsed: Duration,
    /// Passes run (none: the build was as the last one left it).
    passes: usize,
    /// The commands its passes ran, and the job's.
    ran: u64,
    total: u64,
}

/// The errors and warnings of `end`, counted.
fn counts(end: &End, s: Style) -> Vec<String> {
    let mut out = Vec::new();
    let errors = end.summary.errors.len();
    let warnings = end.summary.warnings();
    if errors > 0 {
        out.push(s.red(&plural(errors, "error", "errors")));
    }
    if warnings > 0 {
        out.push(s.yellow(&plural(warnings, "warning", "warnings")));
    }
    out
}

/// The output of `end`, linked, or its file.
fn subject(end: &End, s: Style) -> String {
    match (&end.output, end.failed || end.checked) {
        (Some(o), false) => s.link(&s.bold(o), o),
        _ => s.bold(end.file),
    }
}

/// The last line of a build: `Finished paper.pdf · 12 pages · …`.
fn result_line(end: &End, t: &Totals, s: Style) -> String {
    let sep = s.sep();
    let mut facts = Vec::new();
    if end.failed {
        facts = counts(end, s);
    }
    facts.push(plural(end.summary.pages, "page", "pages"));
    if let Some(b) = end.bytes.filter(|_| !end.failed) {
        facts.push(size(b));
    }
    if t.passes > 0 {
        facts.push(plural(t.passes, "pass", "passes"));
    }
    facts.push(s.bold(&secs(t.elapsed)));
    let warnings = end.summary.warnings();
    if !end.failed && warnings > 0 {
        facts.push(s.yellow(&plural(warnings, "warning", "warnings")));
    }
    let verb = if end.failed {
        s.red(&format!("{:>12}", "Failed"))
    } else if end.checked {
        s.green(&format!("{:>12}", "Checked"))
    } else {
        s.green(&format!("{:>12}", "Finished"))
    };
    format!("{verb} {}{sep}{}\n", subject(end, s), facts.join(sep))
}

/// What the last build had: a watch's line says what is not the same.
#[derive(Clone, Copy, Default)]
struct Was {
    pages: Option<usize>,
    warnings: Option<usize>,
}

/// A watch's line for a rebuild: `17:03:12 ↻ paper.tex:18 ✓ paper.pdf ·
/// 18 ms · 0.8% run again`; its pages and warnings if they are not as
/// many as the last build's (`was`).
fn rebuild_line(end: &End, rb: &Rebuild, t: &Totals, was: Was, s: Style) -> String {
    let sep = s.sep();
    let mut facts = Vec::new();
    if end.failed {
        facts.push(s.red(&plural(end.summary.errors.len(), "error", "errors")));
    }
    facts.push(s.bold(&secs(t.elapsed)));
    if was.pages != Some(end.summary.pages) {
        facts.push(plural(end.summary.pages, "page", "pages"));
    }
    if t.passes > 1 {
        facts.push(plural(t.passes, "pass", "passes"));
    }
    if t.total > 0 {
        #[allow(clippy::cast_precision_loss)]
        let pct = t.ran as f64 * 100.0 / t.total as f64;
        let pct = if pct < 0.1 && t.ran > 0 {
            String::from("<0.1")
        } else if pct < 10.0 {
            format!("{pct:.1}")
        } else {
            format!("{pct:.0}")
        };
        facts.push(s.dim(&format!("{pct}% run again")));
    }
    let w = end.summary.warnings();
    if was.warnings.is_none_or(|was| was != w) && (w > 0 || was.warnings.is_some()) {
        facts.push(s.yellow(&plural(w, "warning", "warnings")));
    }
    let mark = if end.failed {
        s.red(s.sym("✗", "x"))
    } else {
        s.green(s.sym("✓", "ok"))
    };
    let changed = if rb.changed.is_empty() {
        String::new()
    } else {
        format!(
            "{} {} ",
            s.cyan(s.sym("↻", "*")),
            s.cyan(&rb.changed.join(", "))
        )
    };
    format!(
        "{} {changed}{mark} {}{sep}{}\n",
        s.dim(&format!("{:>12}", clock_time(SystemTime::now()))),
        subject(end, s),
        facts.join(sep)
    )
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.live.stop();
    }
}

/// How many items of a group the terminal shows (`partex why` shows all).
const SHOWN: usize = 3;

/// A group of warnings.
#[must_use]
pub fn group(g: &Group, s: Style) -> String {
    let mut out = String::new();
    if g.kind == crate::warnings::Kind::Other {
        // (each says something else: a headline each)
        for item in g.items.iter().take(SHOWN) {
            let locs: Vec<String> = item.locations.iter().map(ToString::to_string).collect();
            let _ = writeln!(
                out,
                "{}: {}",
                s.yellow("warning"),
                s.bold(&clip(&headline(&item.subject), 100))
            );
            let _ = writeln!(out, "{} {}", s.blue("  -->"), locs.join(", "));
        }
        if g.items.len() > SHOWN {
            let _ = writeln!(
                out,
                "{} {} more (`partex why`)",
                s.blue("   ="),
                plural(g.items.len() - SHOWN, "warning", "warnings")
            );
        }
        return out;
    }
    let _ = writeln!(out, "{}: {}", s.yellow("warning"), s.bold(&g.title()));
    let n = g.items.len();
    // (the worst boxes first)
    let mut items: Vec<_> = g.items.iter().collect();
    if g.kind.code().starts_with("overfull") {
        items.sort_by_key(|i| std::cmp::Reverse(i.amount));
    }
    for item in items.iter().take(SHOWN) {
        let locs: Vec<String> = item.locations.iter().map(ToString::to_string).collect();
        let arrow = s.blue("  -->");
        let _ = writeln!(out, "{arrow} {}", locs.join(", "));
        let bar = s.blue("   |");
        if !item.excerpt.is_empty() {
            let _ = writeln!(out, "{bar} {}", clip(&item.excerpt, 76));
        }
        match g.kind.code() {
            "undefined-reference" | "undefined-citation" => {}
            _ => {
                let _ = writeln!(out, "{bar} {}", clip(&item.subject, 100));
            }
        }
    }
    if n > SHOWN {
        let more: Vec<String> = items[SHOWN..]
            .iter()
            .flat_map(|i| i.locations.iter().map(ToString::to_string))
            .take(4)
            .collect();
        let etc = if n - SHOWN > more.len() { ", …" } else { "" };
        let _ = writeln!(
            out,
            "{} {} more: {}{etc}",
            s.blue("   ="),
            n - SHOWN,
            more.join(", ")
        );
    }
    out
}

/// A package's or class's warning as a headline: `Package lipsum
/// Warning: Unknown language` is `lipsum: Unknown language`.
fn headline(text: &str) -> String {
    for lead in ["Package ", "Class "] {
        if let Some(rest) = text.strip_prefix(lead)
            && let Some((who, what)) = rest.split_once(" Warning: ")
            && !who.contains(' ')
        {
            return format!("{who}: {what}");
        }
    }
    text.strip_prefix("LaTeX Warning: ")
        .unwrap_or(text)
        .to_owned()
}

/// `text` cut to `max` characters.
fn clip(text: &str, max: usize) -> String {
    let one_line = text.replace('\n', " ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let cut: String = one_line.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warnings::{Item, Kind, Loc};

    #[test]
    fn a_group_shows_its_worst_and_counts_the_rest() {
        let item = |line, amount| Item {
            subject: format!("Overfull \\hbox in paragraph at lines {line}--{line}"),
            locations: vec![Loc {
                file: "ch02.tex".into(),
                line,
            }],
            amount,
            excerpt: "the Nesterov accelerated gradient".into(),
        };
        let g = Group {
            kind: Kind::OverfullHbox,
            items: vec![
                item(1, 65536),
                item(2, 3 * 65536),
                item(3, 2 * 65536),
                item(4, 32768),
                item(5, 32768),
            ],
        };
        let text = group(&g, Style::plain());
        assert_eq!(
            text,
            "warning: 5 overfull \\hboxes (worst 3.0pt)\n\
             \x20 --> ch02.tex:2\n\
             \x20  | the Nesterov accelerated gradient\n\
             \x20  | Overfull \\hbox in paragraph at lines 2--2\n\
             \x20 --> ch02.tex:3\n\
             \x20  | the Nesterov accelerated gradient\n\
             \x20  | Overfull \\hbox in paragraph at lines 3--3\n\
             \x20 --> ch02.tex:1\n\
             \x20  | the Nesterov accelerated gradient\n\
             \x20  | Overfull \\hbox in paragraph at lines 1--1\n\
             \x20  = 2 more: ch02.tex:4, ch02.tex:5\n"
        );
        let color = Style {
            color: true,
            ..Style::plain()
        };
        assert!(group(&g, color).contains("\x1b[1;33mwarning\x1b[0m"));
    }

    #[test]
    fn other_warnings_are_headlines() {
        let g = Group {
            kind: Kind::Other,
            items: vec![Item {
                subject: "Package lipsum Warning: Unknown language 'latin'.".into(),
                locations: vec![Loc {
                    file: "paper.tex".into(),
                    line: 8,
                }],
                amount: 0,
                excerpt: String::new(),
            }],
        };
        assert_eq!(
            group(&g, Style::plain()),
            "warning: lipsum: Unknown language 'latin'.\n  --> paper.tex:8\n"
        );
        assert_eq!(
            headline("LaTeX Warning: Label(s) may have changed."),
            "Label(s) may have changed."
        );
    }

    #[test]
    fn numbers_sizes_and_times() {
        assert_eq!(thousands(98_803), "98,803");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(999), "999");
        assert_eq!(compact(812), "812");
        assert_eq!(compact(12_340), "12.3k");
        assert_eq!(compact(12_000), "12k");
        assert_eq!(compact(1_240_000), "1.24M");
        assert_eq!(size(812), "812 B");
        assert_eq!(size(86_000), "84 KB");
        assert_eq!(size(5_000), "4.9 KB");
        assert_eq!(size(1_300_000), "1.2 MB");
        assert_eq!(secs(Duration::from_millis(38)), "38 ms");
        assert_eq!(secs(Duration::from_millis(1234)), "1.23 s");
        assert_eq!(secs(Duration::from_millis(12_345)), "12.3 s");
        assert_eq!(secs(Duration::from_secs(125)), "2 min 05 s");
        assert_eq!(plural(1, "page", "pages"), "1 page");
        assert_eq!(plural(1295, "page", "pages"), "1,295 pages");
    }

    #[test]
    fn estimates_round_trip() {
        let e = Estimate {
            commands: 98_803,
            pages: 12,
            millis: 1310,
        };
        assert_eq!(Estimate::parse(&e.to_text()), Some(e));
        assert_eq!(Estimate::parse("commands 0\n"), None);
        assert_eq!(Estimate::parse("garbage"), None);
    }
}
