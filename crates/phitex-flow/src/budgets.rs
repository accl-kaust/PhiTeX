//! The edit budgets (`AGENTS_BRIEF.md`, "Budgets"), measured: a word edit
//! ≤ 50 ms and ≤ 300 steps; an inserted line ≤ 50 ms; an edit that moves a
//! page break ≤ 100 ms; no edit more than 25% of a cold build. Over the
//! plain documents of the fuzz corpus, "big" (every body four times, 48
//! pages) and "bigout" (twelve times with a mark per part, about 150 pages
//! through plain's output routine, its marks and footnotes), each edit
//! kind applied to the document's own text (and taken back, unmeasured)
//! `reps` times at each schedule: the median and the maximum, the nodes
//! (steps) and pieces run again, the join's time. Informational:
//! `phitex-flow --budgets`, `scripts/flow-budgets.sh`, `tests/budgets.rs`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crate::build::{Options, Order, Output, Reads};
use crate::fuzz::{self, Doc, Kind};

/// The kinds of edit measured: the brief's three, and two of ours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    /// A word of a paragraph in the middle replaced by another as long.
    Word,
    /// A line of words inserted in a paragraph in the middle.
    Line,
    /// A paragraph of six lines inserted near the start: every page break
    /// after it moves.
    PageBreak,
    /// A macro every paragraph of the body reads, defined again (the
    /// document with `\fz` at the start of each of its paragraphs).
    Def,
    /// A global parameter at the start of the body (`\parindent`).
    Param,
}

impl EditKind {
    pub const ALL: [EditKind; 5] = [EditKind::Word, EditKind::Line, EditKind::PageBreak, EditKind::Def, EditKind::Param];

    pub fn name(self) -> &'static str {
        match self {
            EditKind::Word => "word",
            EditKind::Line => "line",
            EditKind::PageBreak => "page-break",
            EditKind::Def => "def",
            EditKind::Param => "param",
        }
    }

    /// The brief's budget: milliseconds, and steps.
    pub fn budget(self) -> (Option<u64>, Option<u64>) {
        match self {
            EditKind::Word => (Some(50), Some(300)),
            EditKind::Line => (Some(50), None),
            EditKind::PageBreak => (Some(100), None),
            EditKind::Def | EditKind::Param => (None, None),
        }
    }
}

/// A schedule measured.
#[derive(Clone, Copy, Debug)]
pub struct Sched {
    pub threads: usize,
    pub lean: bool,
}

/// One edit kind on one document at one schedule.
#[derive(Clone, Debug)]
pub struct Row {
    pub doc: String,
    pub pages: usize,
    pub kind: EditKind,
    pub sched: Sched,
    pub cold: Duration,
    /// Each repetition's update time, sorted.
    pub times: Vec<Duration>,
    pub steps: Vec<u64>,
    pub pieces: Vec<usize>,
    pub join: Vec<Duration>,
    /// Each repetition's longest phase (`Stats::time_*`): its name, its time.
    pub phase: Vec<(&'static str, Duration)>,
    /// The budgets this edit missed, said.
    pub misses: Vec<String>,
}

fn median<T: Copy + Ord>(v: &[T]) -> T {
    let mut w = v.to_vec();
    w.sort_unstable();
    w[w.len() / 2]
}

fn max<T: Copy + Ord + Default>(v: &[T]) -> T {
    v.iter().copied().max().unwrap_or_default()
}

/// The budget documents: the plain ones of the corpus, big and bigout.
pub fn docs() -> Vec<Doc> {
    let mut v: Vec<Doc> = fuzz::corpus().into_iter().filter(|d| d.kind == Kind::Plain).collect();
    if let Some(body) = fuzz::big_body(12) {
        // (a mark at each part: the headlines and the marks change with it)
        let body = body.replace("\\message{[part ", "\\mark{part}\\message{[part ");
        if let Some(d) = fuzz::plain_body_doc("bigout", &body) {
            v.push(d);
        }
    }
    v
}

/// The paragraphs of the body as (start, end) byte ranges, end before the
/// blank line.
fn paragraphs(text: &str, from: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = from;
    for chunk in text[from..].split("\n\n") {
        let s = at;
        at += chunk.len() + 2;
        let t = chunk.trim_start_matches('\n');
        let s = s + (chunk.len() - t.len());
        if !t.trim().is_empty() {
            out.push((s, s + t.len()));
        }
    }
    out
}

/// A paragraph of running text (a line of it beginning with a letter) at
/// or after `frac` of the body.
fn text_paragraph(doc: &Doc, text: &str, frac: f64) -> Option<(usize, usize)> {
    let ps = paragraphs(text, doc.body);
    let n = ps.len();
    // (from the place asked for, the first that holds running text)
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let first = ((n as f64) * frac) as usize;
    (first..n).chain(0..first).map(|i| ps[i]).find(|&(s, e)| {
        text[s..e].lines().any(|l| l.chars().next().is_some_and(char::is_alphabetic) && l.split_whitespace().count() > 4)
    })
}

/// The text with edit `kind` number `rep` made (`None`: no place for it).
fn edited(doc: &Doc, text: &str, kind: EditKind, rep: usize) -> Option<String> {
    const WORDS: [&str; 5] = ["alpha", "bravo", "delta", "gamma", "omega"];
    let mut t = text.to_string();
    match kind {
        EditKind::Word | EditKind::Line => {
            let (s, e) = text_paragraph(doc, text, 0.5)?;
            // (a line of running text in it)
            let mut off = s;
            let line = text[s..e].split_inclusive('\n').find_map(|l| {
                let here = off;
                off += l.len();
                (l.chars().next().is_some_and(char::is_alphabetic) && l.split_whitespace().count() > 4).then_some((here, l))
            })?;
            if kind == EditKind::Line {
                let at = line.0 + line.1.len();
                t.insert_str(at, &format!("and here a line of {} words, inserted in the paragraph\n", WORDS[rep % 5]));
            } else {
                // (its second word of five letters or more, replaced)
                let mut pos = line.0;
                let w = line.1.split(' ').find_map(|w| {
                    let here = pos;
                    pos += w.len() + 1;
                    // (its letters, a stop or comma after them left)
                    let w = w.trim_end_matches(['.', ',', ';', '\n']);
                    (w.len() >= 5 && w.chars().all(char::is_alphabetic)).then_some((here, w))
                })?;
                let new: String = WORDS[rep % 5].chars().cycle().take(w.1.len()).collect();
                t.replace_range(w.0..w.0 + w.1.len(), &new);
            }
        }
        EditKind::PageBreak => {
            let (s, _) = text_paragraph(doc, text, 0.1)?;
            let mut p = String::new();
            for k in 0..6 {
                let _ = writeln!(p, "This inserted paragraph, number {rep}, line {k}, is long enough to fill a line of text.");
            }
            p.push('\n');
            t.insert_str(s, &p);
        }
        EditKind::Def if doc.kind.etex() => {
            // (a LaTeX document: its first \newcommand's body)
            let at = doc.body + t[doc.body..].find("\\newcommand{")?;
            let close = at + t[at..].find("}{")? + 2;
            let n = t[close..].find('}')?;
            t.replace_range(close..close + n, WORDS[rep % 5]);
        }
        EditKind::Def => {
            let at = t[doc.body..].find("\\def\\fz{lazy}").map(|i| doc.body + i)?;
            t.replace_range(at..at + "\\def\\fz{lazy}".len(), &format!("\\def\\fz{{{}}}", WORDS[rep % 5]));
        }
        EditKind::Param if doc.kind.etex() => {
            // (a LaTeX document: `\parskip`, after `\begin{document}`)
            let at = doc.body + t[doc.body..].find("\\begin{document}\n")? + "\\begin{document}\n".len();
            t.insert_str(at, &format!("\\setlength{{\\parskip}}{{{}pt}}\n", 1 + rep));
        }
        EditKind::Param => {
            t.insert_str(doc.body, &format!("\\parindent={}pt\n", 21 + rep));
        }
    }
    Some(t)
}

/// The document `def` edits: `\fz` at the start of each running-text
/// paragraph of the body (so every paragraph reads it).
fn with_fz(doc: &Doc) -> Doc {
    let mut d = doc.clone();
    let mut t = d.text.clone();
    let mut ps = paragraphs(&t, d.body);
    ps.reverse();
    for (s, e) in ps {
        if t[s..e].chars().next().is_some_and(char::is_alphabetic) {
            t.insert_str(s, "\\fz{} ");
        }
    }
    d.text = t;
    d
}

fn opts(threads: usize, etex: bool) -> Options {
    Options { threads, reads: Reads::Speculate, order: Order::Priority, dump: false, etex, ..Options::default() }
}

fn timed(f: impl FnOnce() -> Output) -> (Output, Duration) {
    let t = Instant::now();
    let o = f();
    (o, t.elapsed())
}

/// Measures every kind on `doc` at `sched`, `reps` times each.
pub fn measure(doc: &Doc, sched: Sched, reps: usize, kinds: &[EditKind]) -> Vec<Row> {
    let mut rows = Vec::new();
    let latex = doc.kind.etex();
    let o = opts(sched.threads, latex);
    // (a first build, not measured: the fonts read, the caches warm)
    fuzz::builder(doc, &doc.files).update(&doc.text, o);
    for &kind in kinds {
        let d = if kind == EditKind::Def && !latex { with_fz(doc) } else { doc.clone() };
        let mut b = fuzz::builder(&d, &d.files);
        b.set_lean(sched.lean);
        let (first, cold) = timed(|| b.update(&d.text, o));
        let pages = first.stats.pages;
        let mut row = Row {
            doc: doc.name.clone(),
            pages,
            kind,
            sched,
            cold,
            times: Vec::new(),
            steps: Vec::new(),
            pieces: Vec::new(),
            join: Vec::new(),
            phase: Vec::new(),
            misses: Vec::new(),
        };
        for rep in 0..reps {
            let Some(t) = edited(&d, &d.text, kind, rep) else { break };
            let (e, dt) = timed(|| b.update(&t, o));
            row.times.push(dt);
            row.steps.push(e.stats.commands_run);
            row.pieces.push(e.stats.ran);
            row.join.push(e.stats.time_join);
            let s = &e.stats;
            let phases = [
                ("run", s.time_run),
                ("break", s.time_break),
                ("scan", s.time_scan),
                ("ship", s.time_ship),
                ("log", s.time_log),
                ("confirm", s.time_confirm),
                ("held", s.time_held),
                ("cut", s.time_cut),
                ("setup", s.time_setup),
                ("join", s.time_join),
            ];
            row.phase.push(phases.into_iter().max_by_key(|p| p.1).unwrap_or(("-", Duration::ZERO)));
            // (back to the document, not measured)
            b.update(&d.text, o);
        }
        if row.times.is_empty() {
            continue;
        }
        let (ms, steps) = kind.budget();
        let worst = max(&row.times);
        if let Some(ms) = ms
            && worst > Duration::from_millis(ms)
        {
            row.misses.push(format!("{} ms > {ms} ms", worst.as_millis()));
        }
        if let Some(st) = steps
            && max(&row.steps) > st
        {
            row.misses.push(format!("{} steps > {st}", max(&row.steps)));
        }
        if worst * 4 > row.cold {
            row.misses.push(format!("{} ms > 25% of the cold {} ms", worst.as_millis(), row.cold.as_millis()));
        }
        // (progress, on stderr: a LaTeX table takes minutes)
        eprintln!("budgets: {} {} -j{}{}: max {} ms, cold {} ms", row.doc, kind.name(), sched.threads, if sched.lean { " lean" } else { "" }, worst.as_millis(), row.cold.as_millis());
        rows.push(row);
    }
    rows
}

/// The table: one line per row, the budget misses flagged.
pub fn table(rows: &[Row]) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{:<8} {:>5} {:<10} {:>2} {:<5} {:>8} {:>8} {:>8} {:>7} {:>7} {:>6} {:>8} {:>14}  budget",
        "doc", "pages", "edit", "W", "lean", "cold ms", "med ms", "max ms", "steps", "max st", "pieces", "join ms", "longest phase"
    );
    let ms = |d: Duration| format!("{:.1}", d.as_secs_f64() * 1000.0);
    for r in rows {
        let _ = writeln!(
            s,
            "{:<8} {:>5} {:<10} {:>2} {:<5} {:>8} {:>8} {:>8} {:>7} {:>7} {:>6} {:>8} {:>14}  {}",
            r.doc,
            r.pages,
            r.kind.name(),
            r.sched.threads,
            if r.sched.lean { "lean" } else { "nodes" },
            ms(r.cold),
            ms(median(&r.times)),
            ms(max(&r.times)),
            median(&r.steps),
            max(&r.steps),
            median(&r.pieces),
            ms(median(&r.join)),
            {
                // (the median repetition's)
                let mut i: Vec<usize> = (0..r.times.len()).collect();
                i.sort_by_key(|&k| r.times[k]);
                r.phase.get(i[i.len() / 2]).map_or(String::new(), |(n, d)| format!("{n} {}", ms(*d)))
            },
            if r.misses.is_empty() { "ok".to_string() } else { format!("MISS: {}", r.misses.join("; ")) }
        );
    }
    let misses = rows.iter().filter(|r| !r.misses.is_empty()).count();
    let _ = writeln!(s, "{misses} of {} rows miss a budget", rows.len());
    s
}

/// Every document at -j1 and -j8, lean and not.
pub fn run_all(reps: usize, only: Option<&str>) -> Vec<Row> {
    let mut rows = Vec::new();
    // (the LaTeX documents, `latex` for all of them on one format,
    // `latex-NAME` for one: at 8 workers, lean)
    let latex = Sched { threads: 8, lean: true };
    if only == Some("latex") {
        return fuzz::latex_corpus().iter().flat_map(|d| measure(d, latex, reps, &EditKind::ALL)).collect();
    }
    if let Some(d) = only.filter(|n| n.starts_with("latex-")).and_then(fuzz::find_doc) {
        return measure(&d, latex, reps, &EditKind::ALL);
    }
    for d in docs().iter().filter(|d| only.is_none_or(|n| n == d.name)) {
        for threads in [1, 8] {
            for lean in [true, false] {
                rows.extend(measure(d, Sched { threads, lean }, reps, &EditKind::ALL));
            }
        }
    }
    rows
}
