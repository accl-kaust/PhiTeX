//! The modern command line's terminal (DESIGN.md, "Command line and
//! terminal"): quiet, coloured status lines like cargo's, live progress
//! from real events, errors as rustc-style snippets and warnings grouped at
//! the end. Everything goes to standard error; the TeX transcript still
//! goes, whole, to the `.log` file.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::events::{Live, LiveSink, Progress};
use crate::warnings::{Group, Summary};

/// Colours, or none.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
}

impl Style {
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
}

/// How much to show.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub style: Style,
    /// Redraw a live progress line (a terminal).
    pub progress: bool,
    /// 0: status and problems; 1: also what the document shows on the
    /// terminal (`\message`, `\typeout`); 2: also TeX's raw terminal
    /// stream.
    pub verbose: u8,
    /// Only problems and the final line.
    pub quiet: bool,
}

/// The live line: the pass running and the pages it has shipped.
struct LiveLine {
    settings: Settings,
    pass: usize,
    pages: usize,
    /// The last page's `\count0`.
    last: i32,
    started: Instant,
    drawn: Option<Instant>,
}

impl LiveLine {
    fn draw(&mut self, force: bool) {
        if !self.settings.progress || self.settings.quiet || self.pass == 0 {
            return;
        }
        let now = Instant::now();
        if !force
            && self
                .drawn
                .is_some_and(|t| now - t < Duration::from_millis(80))
        {
            return;
        }
        self.drawn = Some(now);
        let s = self.settings.style;
        let pages = if self.pages == 0 {
            String::new()
        } else {
            format!("  {} [{}]", plural(self.pages, "page", "pages"), self.last)
        };
        let mut e = std::io::stderr().lock();
        let _ = write!(
            e,
            "\r\x1b[2K{} {}{}  {}",
            s.cyan(&format!("{:>12}", format!("Pass {}", self.pass))),
            spinner(now - self.started),
            pages,
            s.dim(&secs(now - self.started))
        );
        let _ = e.flush();
    }

    fn clear(&mut self) {
        if self.drawn.take().is_some() {
            let mut e = std::io::stderr().lock();
            let _ = write!(e, "\r\x1b[2K");
            let _ = e.flush();
        }
    }
}

fn spinner(t: Duration) -> char {
    const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
    FRAMES[usize::try_from(t.as_millis() / 120).unwrap_or(0) % 4]
}

/// `n` things.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A duration as people read it: `0.21 s`, `38 ms`.
pub fn secs(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{:.0} ms", s * 1e3)
    } else {
        format!("{s:.2} s")
    }
}

/// The renderer of one command.
pub struct Renderer {
    pub settings: Settings,
    live: Arc<Mutex<LiveLine>>,
    started: Instant,
    passes: usize,
}

impl Renderer {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            live: Arc::new(Mutex::new(LiveLine {
                settings,
                pass: 0,
                pages: 0,
                last: 0,
                started: Instant::now(),
                drawn: None,
            })),
            started: Instant::now(),
            passes: 0,
        }
    }

    fn style(&self) -> Style {
        self.settings.style
    }

    fn with_live<R>(&self, f: impl FnOnce(&mut LiveLine) -> R) -> R {
        let mut l = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut l)
    }

    /// A status line: `   Compiling paper.tex (pdflatex)`.
    /// Whether `-v` was given.
    pub fn verbose(&self) -> bool {
        self.settings.verbose > 0
    }

    pub fn status(&self, verb: &str, what: &str) {
        if self.settings.quiet {
            return;
        }
        self.line(&format!(
            "{} {what}",
            self.style().green(&format!("{verb:>12}"))
        ));
    }

    /// A line on standard error, below the live line.
    pub fn line(&self, text: &str) {
        self.with_live(|l| {
            l.clear();
            eprintln!("{text}");
            l.draw(true);
        });
    }

    /// Where live progress goes.
    pub fn live_sink(&self) -> LiveSink {
        let live = self.live.clone();
        Arc::new(move |e: Live| {
            let mut l = live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match e {
                Live::Page(n) => {
                    l.pages += 1;
                    l.last = n;
                    l.draw(false);
                }
            }
        })
    }

    /// A build begins.
    pub fn start(&mut self) {
        self.started = Instant::now();
        self.passes = 0;
    }

    /// What a converging build did.
    pub fn progress(&mut self, p: &Progress) {
        let s = self.style();
        match p {
            Progress::PassStart(n) => self.with_live(|l| {
                l.pass = *n;
                l.pages = 0;
                l.started = Instant::now();
                l.draw(true);
            }),
            Progress::Pass(n, report) => {
                let pages = self.with_live(|l| {
                    l.clear();
                    l.pass = 0;
                    l.pages
                });
                let Some(r) = report else {
                    if *n == 1 {
                        self.status("Fresh", "nothing changed since the last build");
                    }
                    return;
                };
                self.passes = *n;
                let mut what = Vec::new();
                if r.commands < r.total_commands {
                    what.push(format!(
                        "resumed, {} of {} commands run",
                        r.commands, r.total_commands
                    ));
                } else {
                    what.push(format!("{} commands run", r.total_commands));
                }
                if let Some(c) = r.cut_at {
                    what.push(format!("converged early at command {c}"));
                }
                if pages > 0 {
                    what.push(plural(pages, "page", "pages"));
                }
                if !self.settings.quiet {
                    self.line(&format!(
                        "{} {} {}",
                        s.cyan(&format!("{:>12}", format!("Pass {n}"))),
                        what.join(", "),
                        s.dim(&format!("({})", secs(r.elapsed)))
                    ));
                    if self.settings.verbose > 0 {
                        for w in &r.why {
                            self.line(&format!("{:>12} {}", "", s.dim(w)));
                        }
                    }
                }
            }
            Progress::Tool(line) => {
                let line = line.strip_prefix("partex: ").unwrap_or(line);
                let (tool, rest) = line.split_once(' ').unwrap_or((line, ""));
                if !self.settings.quiet {
                    self.line(&format!("{} {rest}", s.blue(&format!("{tool:>12}"))));
                }
            }
        }
    }

    /// The end of a build: problems, then one line on the result.
    pub fn finish(&self, end: &End) {
        let s = self.style();
        self.with_live(LiveLine::clear);
        let mut e = std::io::stderr().lock();
        if self.settings.verbose >= 2 && !end.term.is_empty() {
            let _ = e.write_all(end.term);
            if !end.term.ends_with(b"\n") {
                let _ = writeln!(e);
            }
        }
        if self.settings.verbose >= 1 {
            for m in &end.summary.messages {
                let _ = writeln!(e, "{} {m}", s.dim("message:"));
            }
        }
        // (the same error at the same place, once, with a count)
        let mut errors: Vec<(String, usize)> = Vec::new();
        for d in &end.summary.errors {
            let text = crate::snippet::error(d, s, self.settings.verbose);
            match errors.iter_mut().find(|(t, _)| *t == text) {
                Some((_, n)) => *n += 1,
                None => errors.push((text, 1)),
            }
        }
        for (text, n) in errors {
            let _ = write!(e, "{text}");
            if n > 1 {
                let _ = writeln!(e, "{} {n} times", s.blue("   ="));
            }
            let _ = writeln!(e);
        }
        for g in &end.summary.groups {
            let _ = writeln!(e, "{}", group(g, s));
        }
        let elapsed = secs(self.started.elapsed());
        let passes = plural(self.passes.max(1), "pass", "passes");
        let pages = plural(end.summary.pages, "page", "pages");
        let errors = end.summary.errors.len();
        let warnings = end.summary.warnings();
        let problems = match (errors, warnings) {
            (0, 0) => String::new(),
            (0, w) => format!(", {}", s.yellow(&plural(w, "warning", "warnings"))),
            (e, 0) => format!(", {}", s.red(&plural(e, "error", "errors"))),
            (e, w) => format!(
                ", {}, {}",
                s.red(&plural(e, "error", "errors")),
                s.yellow(&plural(w, "warning", "warnings"))
            ),
        };
        let output = end
            .output
            .as_deref()
            .map(|o| format!(" -> {o}"))
            .unwrap_or_default();
        let verb = if end.failed {
            s.red(&format!("{:>12}", "Failed"))
        } else if end.checked {
            s.green(&format!("{:>12}", "Checked"))
        } else {
            s.green(&format!("{:>12}", "Finished"))
        };
        let _ = writeln!(
            e,
            "{verb} {} ({pages}, {passes}{problems}) in {elapsed}{output}",
            end.file
        );
        if warnings > 0 && !self.settings.quiet {
            let _ = writeln!(
                e,
                "{:>12} {}",
                "",
                s.dim("every warning, and why the build ran as it did: `partex why`")
            );
        }
    }
}

/// What a finished build shows.
pub struct End<'a> {
    pub file: &'a str,
    pub summary: &'a Summary,
    /// TeX's terminal stream (`-vv`).
    pub term: &'a [u8],
    /// The main output file (PDF or DVI).
    pub output: Option<String>,
    pub failed: bool,
    pub checked: bool,
}

/// How many items of a group the terminal shows (`partex why` shows all).
const SHOWN: usize = 3;

/// A group of warnings.
#[must_use]
pub fn group(g: &Group, s: Style) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
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
        let text = group(&g, Style { color: false });
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
        assert!(group(&g, Style { color: true }).contains("\x1b[1;33mwarning\x1b[0m"));
    }
}
