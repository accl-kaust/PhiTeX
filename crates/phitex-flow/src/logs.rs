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

use crate::graph::{Run, paragraph_start};
use crate::item::Item;
use crate::page::{Lines, Page, page_mark};
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
            // (`N` and a node's index: the line that node had reached —
            // where a group or a conditional began, for e-TeX's
            // `\showgroups`, `\showifs`)
            w if w.starts_with('N') => {
                let j = w[1..].parse::<usize>().unwrap_or(i).min(i);
                let _ = write!(out, "{REL}{}{REL}", line_of(text, run, j));
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

/// `s` with its line numbers moved down by `base` lines.
fn rebase(s: &str, base: usize) -> String {
    if !s.contains(REL) {
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
    out
}

/// The log's entries: the pieces' parts, the page builder's calls' pages
/// (`marks`, in order: `page::Scan::marks`) and the pages.
pub fn gather(frags: &[&Frag], marks: &[(u32, u32)], pages: &[Page]) -> Vec<String> {
    let mut out = Vec::new();
    let mut base = 0;
    let mut mi = 0;
    let marks_at = |out: &mut Vec<String>, mi: &mut usize| {
        if let Some(&(a, b)) = marks.get(*mi) {
            for p in &pages[a as usize..(b as usize).min(pages.len())] {
                out.push(page_mark(&p.ship));
            }
        }
        *mi += 1;
    };
    for f in frags {
        for e in &f.entries {
            match e {
                Entry::Text(s) => out.push(rebase(s, base)),
                Entry::Mark => marks_at(&mut out, &mut mi),
            }
        }
        base += f.newlines;
    }
    while mi < marks.len() {
        marks_at(&mut out, &mut mi);
    }
    out
}
