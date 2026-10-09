//! The log, in document order: what each node wrote (`\message`,
//! `\immediate\write`, a box's diagnostic, an explicit `\shipout`'s `[n]`),
//! each paragraph's line diagnostics, and the `[n]` of each page the page
//! builder shipped, after the node whose call shipped it. Line numbers are
//! never in a version: a diagnostic holds a placeholder (`show::LINE`) that
//! is filled here from the node's position in the current text.

use std::fmt::Write as _;

use crate::graph::{Run, paragraph_start};
use crate::item::Item;
use crate::page::{Lines, Page, page_mark};
use crate::show::LINE;

/// The line of the text node `i` of `run` had reached (tex.web's `line`:
/// a line's end character read is still that line's).
fn line_of(text: &[char], run: &Run, i: usize, base: usize) -> usize {
    let p = run.chain.nodes[i].out.pos.min(text.len());
    base + 1 + text[..p.saturating_sub(1)].iter().filter(|&&c| c == '\r').count()
}

/// `s` with its line placeholders filled for node `i`.
fn resolve(s: &str, text: &[char], run: &Run, i: usize, base: usize) -> String {
    if !s.contains(LINE) {
        return s.to_string();
    }
    let mut out = String::new();
    let mut parts = s.split(LINE);
    out.push_str(parts.next().unwrap_or(""));
    while let Some(what) = parts.next() {
        let here = line_of(text, run, i, base);
        match what {
            "P" => {
                let start = paragraph_start(&run.chain, i);
                let _ = write!(out, "{}--{here}", line_of(text, run, start, base));
            }
            _ => out.push_str(&here.to_string()),
        }
        out.push_str(parts.next().unwrap_or(""));
    }
    out
}

/// A piece as the log reads it: its text, its run, its paragraphs' lines.
pub type PieceLog<'a> = (&'a [char], &'a Run, &'a [Option<Lines>]);

/// The log's entries for `pieces` (each: its text, its run, its
/// paragraphs' lines), the page builder's calls' pages (`marks`, in
/// order: `page::Scan::marks`) and the pages.
pub fn gather(pieces: &[PieceLog<'_>], marks: &[(u32, u32)], pages: &[Page]) -> Vec<String> {
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
    for &(text, run, lines) in pieces {
        let mut pi = 0;
        for (i, n) in run.chain.nodes.iter().enumerate() {
            if let Some(l) = &n.log {
                out.push(resolve(l, text, run, i, base));
            }
            let Some(p) = &n.para else { continue };
            for it in &p.before {
                if matches!(it, Item::BuildPage(_) | Item::End(_)) {
                    marks_at(&mut out, &mut mi);
                }
            }
            if p.lb.is_some()
                && let Some(Some(ls)) = lines.get(pi)
            {
                for l in ls.iter() {
                    if let Some(r) = &l.report {
                        out.push(resolve(r, text, run, i, base));
                    }
                }
            }
            for it in &p.after {
                if matches!(it, Item::BuildPage(_) | Item::End(_)) {
                    marks_at(&mut out, &mut mi);
                }
            }
            pi += 1;
        }
        base += text.iter().filter(|&&c| c == '\r').count();
    }
    while mi < marks.len() {
        marks_at(&mut out, &mut mi);
    }
    out
}
