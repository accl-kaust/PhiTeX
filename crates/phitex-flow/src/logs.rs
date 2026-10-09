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

use crate::graph::{Run, enclosing_open, opener, paragraph_start, paragraph_start_at};
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
    Mark,
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

/// A piece's part of the log, and the run and lines it was made from.
#[derive(Debug)]
pub struct Frag {
    run: Arc<Run>,
    lines: Vec<Option<Lines>>,
    pub entries: Vec<Entry>,
    /// The lines of the piece's text.
    pub newlines: usize,
}

impl Frag {
    /// Was it made from this run and these lines (the same `Arc`s)?
    pub fn holds(&self, run: &Arc<Run>, lines: &[Option<Lines>]) -> bool {
        Arc::ptr_eq(&self.run, run)
            && self.lines.len() == lines.len()
            && self.lines.iter().zip(lines).all(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            })
    }
}

/// The part of the log of a piece: its text, its run, its paragraphs'
/// lines.
pub fn fragment(text: &[char], run: &Arc<Run>, lines: &[Option<Lines>]) -> Frag {
    let mut entries = Vec::new();
    let mut pi = 0;
    let mark = |entries: &mut Vec<Entry>, items: &[Item]| {
        for it in items {
            if matches!(it, Item::BuildPage(_) | Item::End(_)) {
                entries.push(Entry::Mark);
            }
        }
    };
    for (i, n) in run.chain.nodes.iter().enumerate() {
        if let Some(l) = n.log() {
            entries.push(Entry::Text(resolve(l, text, run, i)));
        }
        let Some(p) = n.para() else { continue };
        mark(&mut entries, &p.before);
        if p.lb.is_some()
            && let Some(Some(ls)) = lines.get(pi)
        {
            for l in ls.iter() {
                if let Some(r) = &l.report {
                    entries.push(Entry::Text(resolve(r, text, run, i)));
                }
            }
        }
        mark(&mut entries, &p.after);
        pi += 1;
    }
    Frag {
        run: run.clone(),
        lines: lines.to_vec(),
        entries,
        newlines: text.iter().filter(|&&c| c == '\r').count(),
    }
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
            Entry::Mark => None,
        })
        .collect()
}

/// The log's entries: the pieces' parts, the page builder's calls' pages
/// (`marks`, in order: `page::Scan::marks`; a call that fired an output
/// routine has its log in `call_logs` instead) and the pages.
pub fn gather(parts: &[Part<'_>], marks: &[(u32, u32)], call_logs: &[(u32, Vec<CallLog>)], pages: &[Page]) -> Vec<String> {
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
            for p in &pages[a as usize..(b as usize).min(pages.len())] {
                out.push(page_mark(&p.ship));
            }
        }
        *mi += 1;
    };
    for part in parts {
        for e in &part.frag.entries {
            match e {
                Entry::Text(s) => out.push(rebase(s, part.base)),
                Entry::Mark => marks_at(&mut out, &mut mi),
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
