//! The log, in document order: what each node wrote (`\message`,
//! `\immediate\write`, a box's diagnostic, an explicit `\shipout`'s `[n]`),
//! each paragraph's line diagnostics, and the `[n]` of each page the page
//! builder shipped, after the node whose call shipped it. Line numbers are
//! never in a version: a diagnostic holds a placeholder (`show::LINE`) that
//! is filled here from the node's position in the current text.

use std::fmt::Write as _;

use crate::graph::{Run, enclosing_open, paragraph_start, paragraph_start_at};
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
            // (where a list began, `\showlists`: the `Open` node of the
            // `k`-th enclosing group, or the paragraph at a level)
            g if g.starts_with('G') => {
                let k: usize = g[1..].parse().unwrap_or(0);
                let l = enclosing_open(&run.chain, i, k).map_or(here, |o| line_of(text, run, o, base));
                out.push_str(&l.to_string());
            }
            x if x.starts_with('X') => context(&mut out, &format!("l.{here} "), &x[1..]),
            x if x.starts_with('T') => {
                let mut f = x[1..].splitn(2, crate::graph::diag::CUT);
                let head = f.next().unwrap_or("");
                context(&mut out, head, f.next().unwrap_or(""));
            }
            q if q.starts_with('Q') => {
                let level: u16 = q[1..].parse().unwrap_or(0);
                let start = paragraph_start_at(&run.chain, i, level);
                out.push_str(&line_of(text, run, start, base).to_string());
            }
            _ => out.push_str(&here.to_string()),
        }
        out.push_str(parts.next().unwrap_or(""));
    }
    out
}

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
