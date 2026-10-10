//! The log, in document order: what each node wrote (`\message`,
//! `\immediate\write`, a box's diagnostic, an explicit `\shipout`'s `[n]`),
//! each paragraph's line diagnostics, and the `[n]` of each page the page
//! builder shipped, after the node whose call shipped it. Line numbers are
//! never in a version: a diagnostic holds a placeholder (`show::LINE`) that
//! is filled from the node's position in the current text.
//!
//! Each piece's part is a value of its own ([`Frag`], made once its run
//! and its paragraphs' lines are final, and kept while they are the same
//! `Arc`s): its entries, its line numbers counted from the piece's first
//! line, and where the page builder's calls stand in it. [`gather`] lays
//! the parts end to end, adding the lines before each piece and filling
//! in the pages each call shipped.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::graph::{Run, alignment_start, enclosing_open, opener, paragraph_start, paragraph_start_at};
use crate::item::Item;
use crate::page::{CallLog, Lines, Page, page_mark};
use crate::show::LINE;

/// The line of the text node `i` of `run` had reached (tex.web's `line`:
/// a line's end character read is still that line's).
fn line_of(text: &[char], run: &Run, i: usize) -> usize {
    let p = run.chain.nodes[i].out.pos.min(text.len());
    1 + text[..p.saturating_sub(1)].iter().filter(|&&c| c == '\r').count()
}

/// Brackets a line number counted from the piece's first line.
const REL: char = '\u{E001}';

/// `s` with its line placeholders filled for node `i` (as line numbers
/// within the piece, bracketed by `REL`).
fn resolve(s: &str, text: &[char], run: &Run, i: usize) -> String {
    if !s.contains(LINE) {
        return s.to_string();
    }
    let mut out = String::new();
    let mut parts = s.split(LINE);
    out.push_str(parts.next().unwrap_or(""));
    while let Some(what) = parts.next() {
        let here = line_of(text, run, i);
        match what {
            "P" => {
                let start = paragraph_start(&run.chain, i);
                let _ = write!(out, "{REL}{}{REL}--{REL}{here}{REL}", line_of(text, run, start));
            }
            // (`E`/`C` and k: the line where the k-th group or conditional
            // open at node `i` (from the outside) began — for e-TeX's
            // `\showgroups`, `\showifs` — read through the node's state now)
            w if w.starts_with('E') || w.starts_with('C') => {
                let k = w[1..].parse::<usize>().unwrap_or(0);
                let j = opener(&run.chain, i, w.starts_with('E'), k).unwrap_or(i).min(i);
                let _ = write!(out, "{REL}{}{REL}", line_of(text, run, j));
            }
            // (an alignment's preamble report: from its `\halign` node)
            "A" => {
                let start = alignment_start(&run.chain, i);
                let _ = write!(out, "{REL}{}{REL}--{REL}{here}{REL}", line_of(text, run, start));
            }
            // (where a list began, `\showlists`: the `Open` node of the
            // `k`-th enclosing group, or the paragraph at a level)
            g if g.starts_with('G') => {
                let k: usize = g[1..].parse().unwrap_or(0);
                let l = enclosing_open(&run.chain, i, k).map_or(here, |o| line_of(text, run, o));
                let _ = write!(out, "{REL}{l}{REL}");
            }
            q if q.starts_with('Q') => {
                let level: u16 = q[1..].parse().unwrap_or(0);
                let start = paragraph_start_at(&run.chain, i, level);
                let _ = write!(out, "{REL}{}{REL}", line_of(text, run, start));
            }
            // (an error's context: laid out once the line number is known,
            // in `rebase`; head, what was read and the rest, cut by `CUT`)
            x if x.starts_with('X') => {
                let _ = write!(out, "{CTX}l.{REL}{here}{REL} {}{}{CTX}", crate::graph::diag::CUT, &x[1..]);
            }
            x if x.starts_with('T') => {
                let _ = write!(out, "{CTX}{}{CTX}", &x[1..]);
            }
            _ => {
                let _ = write!(out, "{REL}{here}{REL}");
            }
        }
        out.push_str(parts.next().unwrap_or(""));
    }
    out
}

/// A log entry of a piece: a text (line numbers within the piece), or a
/// call of the page builder (its pages' `[n]`).
#[derive(Debug)]
pub enum Entry {
    Text(String),
    /// A call of the page builder by node `.0` of the piece.
    Mark(u32),
}

/// Between the entries of one node's log (`Node::log`).
pub const SEP: char = '\u{E007}';

/// Brackets the parts of an event of the output streams in a log entry
/// ([`io_event`]).
pub const IO: char = '\u{E005}';

/// An event of the output streams, as a node's log holds it: `\openout`
/// (`o`, the file's name as scanned), `\write` (`w`, the text written),
/// `\closeout` (`c`), with the stream (0–15; a write's 16 is the terminal
/// and the log, 17 the log only). [`replay`] does them in order.
pub fn io_event(kind: char, stream: u8, payload: &str) -> String {
    format!("{IO}{kind}{stream}{IO}{payload}{IO}")
}

/// Brackets an error's context, laid out by `rebase` (`context`).
const CTX: char = '\u{E002}';

/// TeX Live's `error_line`, `half_error_line` (texmf.cnf).
const ERROR_LINE: usize = 79;
const HALF_ERROR_LINE: usize = 50;

/// tex.web 313–318: an error's context — `head` (`l.N `, or a token
/// list's label), what was read and the rest (cut by `CUT`, each character
/// as it prints): two lines, cut short as TeX does.
fn context(out: &mut String, head: &str, x: &str) {
    use crate::show::LN;
    let (read, rest) = x.split_once(crate::graph::diag::CUT).unwrap_or((x, ""));
    let read: Vec<char> = read.chars().collect();
    let rest: Vec<char> = rest.chars().collect();
    out.push_str(head);
    let l = head.chars().count();
    let first_count = read.len();
    let trick_count = (first_count + 1 + ERROR_LINE - HALF_ERROR_LINE).max(ERROR_LINE);
    let m = rest.len().min(trick_count - first_count);
    let (p, n) = if l + first_count <= HALF_ERROR_LINE {
        (0, l + first_count)
    } else {
        out.push_str("...");
        (l + first_count - HALF_ERROR_LINE + 3, HALF_ERROR_LINE)
    };
    out.extend(&read[p.min(first_count)..]);
    out.push(LN);
    out.extend(std::iter::repeat_n(' ', n));
    let k = if m + n <= ERROR_LINE { m } else { ERROR_LINE - n - 3 };
    out.extend(&rest[..k.min(rest.len())]);
    if m + n > ERROR_LINE {
        out.push_str("...");
    }
}

/// A piece's part of the log but for its paragraphs' lines: made from the
/// run's nodes (which a lean piece then drops, `graph::Lean`).
#[derive(Debug)]
pub struct Plan {
    entries: Vec<PEntry>,
    newlines: usize,
}

#[derive(Debug)]
enum PEntry {
    Text(String),
    /// A call of the page builder by a node.
    Mark(u32),
    /// The reports of the lines of paragraph `pi`, made by node `node`:
    /// the lines (within the piece) where the node was and where its
    /// paragraph began.
    Lines { pi: usize, here: usize, start: usize },
}

/// The plan of the log of a piece: its text, its run (with its nodes).
pub fn plan(text: &[char], run: &Run) -> Plan {
    let mut entries = Vec::new();
    let mut pi = 0;
    let mark = |entries: &mut Vec<PEntry>, items: &[Item], i: usize| {
        for it in items {
            if matches!(it, Item::BuildPage(_) | Item::End(_)) {
                entries.push(PEntry::Mark(i as u32));
            }
        }
    };
    for (i, n) in run.chain.nodes.iter().enumerate() {
        if let Some(l) = n.log() {
            for e in l.split(SEP) {
                entries.push(PEntry::Text(resolve(e, text, run, i)));
            }
        }
        let Some(p) = n.para() else { continue };
        mark(&mut entries, &p.before, i);
        if p.lb.is_some() {
            let start = paragraph_start(&run.chain, i);
            entries.push(PEntry::Lines { pi, here: line_of(text, run, i), start: line_of(text, run, start) });
        }
        mark(&mut entries, &p.after, i);
        pi += 1;
    }
    Plan { entries, newlines: text.iter().filter(|&&c| c == '\r').count() }
}

/// A line report's placeholders filled (as [`resolve`] does, from the
/// lines a [`PEntry::Lines`] keeps).
fn resolve_lines(s: &str, here: usize, start: usize) -> String {
    if !s.contains(LINE) {
        return s.to_string();
    }
    let mut out = String::new();
    let mut parts = s.split(LINE);
    out.push_str(parts.next().unwrap_or(""));
    while let Some(what) = parts.next() {
        match what {
            "P" => {
                let _ = write!(out, "{REL}{start}{REL}--{REL}{here}{REL}");
            }
            x if x.starts_with('X') => {
                let _ = write!(out, "{CTX}l.{REL}{here}{REL} {}{}{CTX}", crate::graph::diag::CUT, &x[1..]);
            }
            x if x.starts_with('T') => {
                let _ = write!(out, "{CTX}{}{CTX}", &x[1..]);
            }
            _ => {
                let _ = write!(out, "{REL}{here}{REL}");
            }
        }
        out.push_str(parts.next().unwrap_or(""));
    }
    out
}

/// A piece's part of the log, and the plan (a lean run's) or the run and
/// the lines it was made from.
#[derive(Debug)]
pub struct Frag {
    plan: Arc<Plan>,
    run: Option<Arc<Run>>,
    lines: Vec<Option<Lines>>,
    pub entries: Vec<Entry>,
    /// The lines of the piece's text.
    pub newlines: usize,
}

impl Frag {
    /// Was it made from this run's plan and these lines (the same `Arc`s)?
    pub fn holds(&self, run: &Arc<Run>, lines: &[Option<Lines>]) -> bool {
        let same = match &run.lean {
            Some(l) => Arc::ptr_eq(&self.plan, &l.plan),
            None => self.run.as_ref().is_some_and(|r| Arc::ptr_eq(r, run)),
        };
        same && self.lines.len() == lines.len()
            && self.lines.iter().zip(lines).all(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            })
    }
}

/// The part of the log of a piece: its text, its run, its paragraphs'
/// lines (a lean run's plan, or one made now).
pub fn fragment(text: &[char], run: &Arc<Run>, lines: &[Option<Lines>]) -> Frag {
    let plan = match &run.lean {
        Some(l) => l.plan.clone(),
        None => Arc::new(plan(text, run)),
    };
    let mut entries = Vec::with_capacity(plan.entries.len());
    for e in &plan.entries {
        match e {
            PEntry::Text(s) => entries.push(Entry::Text(s.clone())),
            PEntry::Mark(i) => entries.push(Entry::Mark(*i)),
            PEntry::Lines { pi, here, start } => {
                if let Some(Some(ls)) = lines.get(*pi) {
                    for l in ls.iter() {
                        if let Some(r) = &l.report {
                            entries.push(Entry::Text(resolve_lines(r, *here, *start)));
                        }
                    }
                }
            }
        }
    }
    let run = run.lean.is_none().then(|| run.clone());
    Frag { newlines: plan.newlines, plan, run, lines: lines.to_vec(), entries }
}

/// `s` with its line numbers moved down by `base` lines, then its errors'
/// contexts laid out (their width depends on the line number).
fn rebase(s: &str, base: usize) -> String {
    if !s.contains(REL) && !s.contains(CTX) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (k, part) in s.split(REL).enumerate() {
        if k % 2 == 1 {
            let n: usize = part.parse().unwrap_or(0);
            let _ = write!(out, "{}", n + base);
        } else {
            out.push_str(part);
        }
    }
    if !out.contains(CTX) {
        return out;
    }
    let mut laid = String::with_capacity(out.len());
    for (k, part) in out.split(CTX).enumerate() {
        if k % 2 == 1 {
            let (head, x) = part.split_once(crate::graph::diag::CUT).unwrap_or((part, ""));
            context(&mut laid, head, x);
        } else {
            laid.push_str(part);
        }
    }
    laid
}

/// A piece's part as the log lays it: its lines before it in its own file
/// (`base`), and what follows it (an `\input`'s `(name`, a file's `)`).
pub struct Part<'a> {
    pub frag: &'a Frag,
    pub base: usize,
    pub post: Option<&'a str>,
}

/// A part's texts as they stand alone (line numbers from 1): an output
/// routine's log.
pub fn texts(f: &Frag) -> Vec<String> {
    f.entries
        .iter()
        .filter_map(|e| match e {
            Entry::Text(s) => Some(rebase(s, 0)),
            Entry::Mark(_) => None,
        })
        .collect()
}

/// The log's entries: the pieces' parts, the page builder's calls' pages
/// (`marks`, in order: `page::Scan::marks`; a call that fired an output
/// routine has its log in `call_logs` instead) and the pages, each page's
/// `[n]` holding the events of its shipping (`page_io[i]` for page `i`:
/// [`crate::graph::page_io`]).
pub fn gather(
    parts: &[Part<'_>],
    marks: &[(u32, u32)],
    call_logs: &[(u32, Vec<CallLog>)],
    pages: &[Page],
    page_io: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut mi = 0;
    let marks_at = |out: &mut Vec<String>, mi: &mut usize| {
        if let Ok(c) = call_logs.binary_search_by_key(&(*mi as u32), |c| c.0) {
            for e in &call_logs[c].1 {
                match e {
                    CallLog::Page(p) => {
                        if let Some(pg) = pages.get(*p as usize) {
                            out.push(page_mark(&pg.ship));
                        }
                    }
                    CallLog::Text(s) => out.push(s.clone()),
                }
            }
        } else if let Some(&(a, b)) = marks.get(*mi) {
            let b = (b as usize).min(pages.len());
            for (i, p) in pages.iter().enumerate().take(b).skip(a as usize) {
                let mut m = page_mark(&p.ship);
                if let Some(io) = page_io.get(i).filter(|s| !s.is_empty()) {
                    m.pop();
                    m.push_str(io);
                    m.push(']');
                }
                out.push(m);
            }
        }
        *mi += 1;
    };
    for part in parts {
        for e in &part.frag.entries {
            match e {
                Entry::Text(s) => out.push(rebase(s, part.base)),
                Entry::Mark(_) => marks_at(&mut out, &mut mi),
            }
        }
        if let Some(p) = part.post {
            out.push(p.to_string());
        }
    }
    while mi < marks.len() {
        marks_at(&mut out, &mut mi);
    }
    out
}

/// Where each page was shipped: the piece and the node after which the
/// page builder's call that shipped it stands (a page shipped at the
/// document's end: after the last piece's last node; an explicit
/// `\shipout`'s page: none, its node did its events).
pub fn page_places(frags: &[&Frag], marks: &[(u32, u32)], pages: usize, end: (usize, usize)) -> Vec<Option<(usize, usize)>> {
    let mut at = vec![None; pages];
    let mut j = 0;
    let mut set = |j: usize, place: (usize, usize)| {
        if let Some(&(a, b)) = marks.get(j) {
            for p in at.iter_mut().take((b as usize).min(pages)).skip(a as usize) {
                *p = Some(place);
            }
        }
    };
    for (k, f) in frags.iter().enumerate() {
        for e in &f.entries {
            if let Entry::Mark(i) = e {
                set(j, (k, *i as usize + 1));
                j += 1;
            }
        }
    }
    while j < marks.len() {
        set(j, end);
        j += 1;
    }
    at
}

/// The log's entries with the output streams' events done, in order
/// (tex.web 1370, 1374): an `\openout` opens its file (the name with
/// `.tex` if it has no extension; emptied) — web2c's `\openout j = `name'.`
/// log line is off for `tex` (texmf.cnf `log_openout.tex = f`); a `\write` to an open stream is a line of its
/// file, to any other a line of the log; `\closeout` closes. Returns the
/// entries and the files written (name, contents), in the order they were
/// first opened.
pub fn replay(entries: Vec<String>) -> (Vec<String>, Vec<(String, String)>) {
    use crate::show::{LN, NL, RAW};
    let mut files: Vec<(String, String)> = Vec::new();
    let mut open: [Option<usize>; 16] = [None; 16];
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        if !e.contains(IO) {
            out.push(e);
            continue;
        }
        let mut s = String::with_capacity(e.len());
        let mut parts = e.split(IO);
        let first = parts.next().unwrap_or("");
        // (an entry with events is raw, its marker at the front)
        let (lead, rest) = match first.find(RAW) {
            Some(i) => (&first[..i], &first[i + RAW.len_utf8()..]),
            None => ("", first),
        };
        s.push_str(lead);
        s.push(RAW);
        s.push_str(rest);
        while let Some(head) = parts.next() {
            let payload = parts.next().unwrap_or("");
            let between = parts.next().unwrap_or("");
            let mut h = head.chars();
            let kind = h.next().unwrap_or('w');
            let j: usize = h.as_str().parse().unwrap_or(17);
            match kind {
                'o' => {
                    if j < 16 {
                        let mut name = payload.to_string();
                        if !name.rsplit('/').next().unwrap_or("").contains('.') {
                            name.push_str(".tex");
                        }
                        let k = if let Some(k) = files.iter().position(|f| f.0 == name) {
                            files[k].1.clear();
                            k
                        } else {
                            files.push((name.clone(), String::new()));
                            files.len() - 1
                        };
                        open[j] = Some(k);
                    }
                }
                'c' => {
                    if j < 16 {
                        open[j] = None;
                    }
                }
                _ => {
                    if let Some(k) = open.get(j).copied().flatten() {
                        files[k].1.push_str(payload);
                        files[k].1.push('\n');
                    } else {
                        s.push(NL);
                        s.extend(payload.chars().map(|c| if c == '\n' { LN } else { c }));
                        s.push(LN);
                    }
                }
            }
            s.push_str(between);
        }
        out.push(s);
    }
    (out, files)
}
