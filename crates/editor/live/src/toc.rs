//! The document's outline from its own sectioning: the `\contentsline`
//! entries LaTeX writes for the `.toc` ([`from_aux`]: every heading the
//! table of contents would show, `\addcontentsline` ones too, at every
//! level, with or without a `\tableofcontents`), each placed
//! where the pages show its heading. The viewer's contents column is made
//! of these when the PDF has no outline of its own (no hyperref, whose
//! bookmarks are the outline when there: [`crate::view::Viewer`]).
//!
//! A heading's place is that of the first glyph, in page order and after
//! the entry before it, whose origin falls in a brace group of the sources
//! holding the entry's title (as written: the `.toc` holds the title's
//! tokens as the `\section` read them): the heading itself, not the table
//! of contents (its glyphs come from the `.toc`) nor a running head. Its
//! point is the glyph's baseline less its font's size, from the page's top.
//! An entry whose title no glyph comes from (a short title, `\section[A]{B}`;
//! a title made by a macro) takes its page from its printed number, counted
//! from the nearest entry placed.

use serde_json::{Value, json};

/// A `\contentsline` of the `.toc`.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// `part`, `chapter`, `section`, ...
    pub level: String,
    /// Its number (`\numberline {N}`; a part's `N\hspace {1em}`).
    pub number: Option<String>,
    /// The title as written to the `.toc`.
    pub title: String,
    /// The page as the document prints it (`\thepage`: "iv", "3").
    pub page: String,
}

/// The `\contentsline`s of `.toc` text `toc`, in order. TeX breaks a
/// written line at `max_print_line` with no character lost: the lines are
/// joined first.
#[must_use]
pub fn lines(toc: &str) -> Vec<Line> {
    let text: String = toc.chars().filter(|&c| c != '\n' && c != '\r').collect();
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("\\contentsline") {
        rest = &rest[i + "\\contentsline".len()..];
        let Some((level, r)) = group(rest) else {
            continue;
        };
        let Some((entry, r)) = group(r) else { continue };
        let Some((page, r)) = group(r) else { continue };
        rest = r;
        let entry = entry.trim();
        let entry = entry
            .strip_prefix("\\protect")
            .map_or(entry, str::trim_start);
        let (number, title) = if let Some(r) = entry.strip_prefix("\\numberline") {
            match group(r) {
                Some((n, t)) => (Some(n.trim().to_owned()), t.to_owned()),
                None => (None, entry.to_owned()),
            }
        } else if level.trim() == "part"
            && let Some((n, t)) = entry.split_once("\\hspace {1em}")
        {
            (Some(n.trim().to_owned()), t.to_owned())
        } else {
            (None, entry.to_owned())
        };
        out.push(Line {
            level: level.trim().to_owned(),
            number,
            title,
            page: page.trim().to_owned(),
        });
    }
    out
}

/// The table of contents' entries as the `.aux` files hold them: each
/// `\@writefile{toc}{...}`'s text, a line each, an `\@input{chap.aux}`'s
/// (an `\include`'s) where it stands, its text from `file`. What LaTeX
/// copies to the `.toc` at the document's end, which it writes only for a
/// `\tableofcontents`: the `.aux` has the entries with or without one.
#[must_use]
pub fn from_aux(aux: &str, file: &dyn Fn(&str) -> Option<String>) -> String {
    fn walk(aux: &str, file: &dyn Fn(&str) -> Option<String>, depth: usize, out: &mut String) {
        let text: String = aux.chars().filter(|&c| c != '\n' && c != '\r').collect();
        let mut rest = text.as_str();
        loop {
            let w = rest.find("\\@writefile");
            let i = rest.find("\\@input");
            match (w, i) {
                (Some(w), i) if i.is_none_or(|i| w < i) => {
                    rest = &rest[w + "\\@writefile".len()..];
                    let Some((list, r)) = group(rest) else {
                        continue;
                    };
                    let Some((entry, r)) = group(r) else { continue };
                    rest = r;
                    if list.trim() == "toc" {
                        out.push_str(entry);
                        out.push('\n');
                    }
                }
                (_, Some(i)) => {
                    rest = &rest[i + "\\@input".len()..];
                    let Some((name, r)) = group(rest) else {
                        continue;
                    };
                    rest = r;
                    if depth < 8
                        && let Some(t) = file(name.trim())
                    {
                        walk(&t, file, depth + 1, out);
                    }
                }
                _ => break,
            }
        }
    }
    let mut out = String::new();
    walk(aux, file, 0, &mut out);
    out
}

/// The brace group `s` begins with (after white space), and what follows it.
fn group(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start().strip_prefix('{')?;
    let mut depth = 1usize;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' => esc = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[..i], &s[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

/// A level's depth: `\part` −1, `\chapter` 0, `\section` 1, ... (another
/// list's or a class's own: as a section).
fn depth(level: &str) -> i8 {
    match level {
        "part" => -1,
        "chapter" => 0,
        "subsection" => 2,
        "subsubsection" => 3,
        "paragraph" => 4,
        "subparagraph" => 5,
        _ => 1,
    }
}

/// A title as compared with the sources: no white space, braces, math
/// shifts or `\protect`s (what `\write` adds or the source may space
/// otherwise: `$\alpha $`, `\LaTeX {}`, `~` written `\nobreakspace {}`).
fn key(s: &str) -> String {
    let s = s
        .replace("\\protect", "")
        .replace("\\relax", "")
        .replace("\\nobreakspace", "~");
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '{' | '}' | '$' | '~'))
        .collect()
}

/// A title shown: its markup's commands dropped, a few as their text.
#[must_use]
pub fn display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cs = s.chars().peekable();
    while let Some(c) = cs.next() {
        match c {
            '\\' => {
                let mut name = String::new();
                while let Some(&n) = cs.peek() {
                    if n.is_ascii_alphabetic() {
                        name.push(n);
                        cs.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() {
                    // (a control symbol: `\&` the character, `\,` a space)
                    match cs.next() {
                        Some(',' | ' ' | ';' | ':') => out.push(' '),
                        Some('\\') => out.push(' '),
                        Some(n) if !n.is_alphanumeric() => out.push(n),
                        _ => {}
                    }
                    continue;
                }
                // (a word's trailing space is the command's, not the text's)
                while cs.peek() == Some(&' ') {
                    cs.next();
                }
                match name.as_str() {
                    "TeX" | "LaTeX" | "LaTeXe" => {
                        out.push_str(if name == "LaTeXe" { "LaTeX2e" } else { &name });
                    }
                    "nobreakspace" | "quad" | "qquad" | "space" => out.push(' '),
                    "ldots" | "dots" | "textellipsis" => out.push('…'),
                    "textendash" => out.push('–'),
                    "textemdash" => out.push('—'),
                    "S" => out.push('§'),
                    _ => {}
                }
            }
            '{' | '}' | '$' => {}
            '~' => out.push(' '),
            '-' if cs.peek() == Some(&'-') => {
                cs.next();
                if cs.peek() == Some(&'-') {
                    cs.next();
                    out.push('—');
                } else {
                    out.push('–');
                }
            }
            '`' if cs.peek() == Some(&'`') => {
                cs.next();
                out.push('“');
            }
            '\'' if cs.peek() == Some(&'\'') => {
                cs.next();
                out.push('”');
            }
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether origins file `name` is one the job wrote and read back (the
/// table of contents' glyphs come from these, never a heading's).
fn written(name: &str) -> bool {
    let n = name.rsplit('/').next().unwrap_or(name);
    [
        ".toc", ".lof", ".lot", ".aux", ".out", ".nav", ".snm", ".loa",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

/// The brace groups of the sources a page's glyphs are set from, where a
/// heading's title may be: each glyph `i` of `o` that starts a run (its
/// glyph before it not the source's text just before it) and the [`key`]
/// of the innermost group its origin falls in, with the group's place
/// (file, `{`). `memo` keeps each group's key.
fn groups(
    o: &phitex_tex::PageOrigins,
    files: &[String],
    texts: &[Option<std::sync::Arc<[u8]>>],
    memo: &mut std::collections::HashMap<(u32, usize), Option<String>>,
    out: &mut Vec<(usize, (u32, usize), String)>,
) {
    for i in 0..o.len() {
        let f = o.file[i];
        if o.is_synth(i) || f == u32::MAX {
            continue;
        }
        let s = o.start[i] as usize;
        let Some(Some(t)) = texts.get(f as usize) else {
            continue;
        };
        // (a run's first glyph only: not one whose glyph before it is the
        // source's text just before it, as within a line, unless a group
        // opens between them: `text.\n\\section*{Index}`)
        if i > 0 && o.file[i - 1] == f && !o.is_synth(i - 1) {
            let e = o.end[i - 1] as usize;
            if e <= s && s - e < 64 && !t.get(e..s).is_some_and(|g| g.contains(&b'{')) {
                continue;
            }
        }
        if files.get(f as usize).is_none_or(|n| written(n)) {
            continue;
        }
        let Some(open) = open_before(t, s) else {
            continue;
        };
        let k = memo.entry((f, open)).or_insert_with(|| {
            close_after(t, open + 1).map(|c| key(&String::from_utf8_lossy(&t[open + 1..c])))
        });
        if let Some(k) = k {
            out.push((i, (f, open), k.clone()));
        }
    }
}

/// The `{` of the innermost brace group holding byte `at` of `t` (within
/// a title's length).
fn open_before(t: &[u8], at: usize) -> Option<usize> {
    let mut depth = 0usize;
    let lo = at.saturating_sub(400);
    let mut i = at.min(t.len());
    while i > lo {
        i -= 1;
        let escaped = i > 0 && t[i - 1] == b'\\';
        match t[i] {
            b'}' if !escaped => depth += 1,
            b'{' if !escaped => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            b'\n' if i > 0 && t[i - 1] == b'\n' => return None,
            _ => {}
        }
    }
    None
}

/// The `}` closing the group whose text starts at `from`.
fn close_after(t: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = from;
    while i < t.len() && i < from + 400 {
        match t[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Where each entry's heading is: its page (from 0) and its top (points
/// from the page's top; none: its page alone known).
///
/// Every glyph run set from a brace group holding an entry's title is a
/// candidate: the heading, but also the table of contents' line (its
/// glyphs are placed in the sources by their text, the title's) or a
/// caption of the same words. The headings are told by their printed page
/// numbers: within the document's arabic numbering, a heading's page less
/// its number is the same for every entry (the pages before page 1), and
/// the contents' lines, all on a page or two, are not. An entry numbered
/// so takes its first candidate at that offset, else the page the offset
/// gives; one numbered otherwise ("iv"), its last candidate between the
/// entries placed around it (a contents' line comes before its heading).
fn place(
    ls: &[Line],
    snap: &phitex_tex::OriginsSnapshot,
    pages: usize,
) -> Vec<(Option<usize>, Option<f64>)> {
    use std::collections::HashMap;
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (j, l) in ls.iter().enumerate() {
        let k = key(&l.title);
        if !k.is_empty() {
            by_key.entry(k).or_default().push(j);
        }
    }
    // (each entry's candidates in page order: page, glyph, top; one a group)
    let mut cands: Vec<Vec<(usize, usize, f64)>> = vec![Vec::new(); ls.len()];
    let mut memo = HashMap::new();
    let mut gs = Vec::new();
    for (k, o) in snap.pages.iter().enumerate() {
        let Some(o) = o else { continue };
        gs.clear();
        groups(o, &snap.files, &snap.texts, &mut memo, &mut gs);
        let mut seen = std::collections::HashSet::new();
        for (i, at, key) in &gs {
            let Some(js) = by_key.get(key) else { continue };
            if !seen.insert(*at) {
                continue;
            }
            let top = f64::from(o.y[*i] - o.size[*i].max(0.0)).max(0.0);
            for &j in js {
                cands[j].push((k, *i, (top * 100.0).round() / 100.0));
            }
        }
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let at = |p: usize| i64::try_from(p).unwrap_or(i64::MAX);
    // (the offset most entries have a candidate at, one vote an entry)
    let mut votes: HashMap<i64, usize> = HashMap::new();
    for (l, cs) in ls.iter().zip(&cands) {
        let Some(n) = num(&l.page) else { continue };
        let ds: std::collections::BTreeSet<i64> = cs.iter().map(|c| at(c.0) - n).collect();
        for d in ds {
            *votes.entry(d).or_default() += 1;
        }
    }
    let offset = votes
        .into_iter()
        .max_by_key(|&(d, v)| (v, std::cmp::Reverse(d.abs())))
        .map(|(d, _)| d);
    let mut placed: Vec<(Option<usize>, Option<f64>)> = vec![(None, None); ls.len()];
    let mut glyph: Vec<Option<(usize, usize)>> = vec![None; ls.len()];
    for (j, l) in ls.iter().enumerate() {
        let Some(n) = num(&l.page) else { continue };
        // (no candidate anywhere: the page its number gives, as from the first page)
        let d = offset.unwrap_or(-1);
        if let Some(&(k, i, y)) = cands[j].iter().find(|c| at(c.0) - n == d) {
            placed[j] = (Some(k), Some(y));
            glyph[j] = Some((k, i));
        } else {
            placed[j].0 = usize::try_from(n + d).ok().filter(|&p| p < pages.max(1));
        }
    }
    for j in 0..ls.len() {
        if num(&ls[j].page).is_some() {
            continue;
        }
        let lo = glyph[..j].iter().rev().find_map(|g| *g);
        let hi = glyph[j + 1..].iter().find_map(|g| *g);
        if let Some(&(k, i, y)) = cands[j]
            .iter()
            .rev()
            .find(|c| lo.is_none_or(|g| (c.0, c.1) > g) && hi.is_none_or(|g| (c.0, c.1) < g))
        {
            placed[j] = (Some(k), Some(y));
            glyph[j] = Some((k, i));
        }
    }
    placed
}

/// The outline of `.toc` text `toc`, placed on the pages of `snap`
/// (`pages` of them in the PDF), as the viewer's entries
/// (`viewer/src/outline.ts`' `Entry`: title, page from 0, its printed
/// number, top in points, entries).
#[must_use]
pub fn outline(toc: &str, snap: &phitex_tex::OriginsSnapshot, pages: usize) -> Value {
    // (the entries nested by level, as the bookmarks are)
    struct Node {
        d: i8,
        v: serde_json::Map<String, Value>,
        kids: Vec<Node>,
    }
    fn close(n: Node) -> Value {
        let mut v = n.v;
        v.insert(
            "k".into(),
            Value::Array(n.kids.into_iter().map(close).collect()),
        );
        Value::Object(v)
    }
    let ls = lines(toc);
    let placed = place(&ls, snap, pages);
    let mut stack: Vec<Node> = Vec::new();
    let mut top: Vec<Value> = Vec::new();
    for (l, (p, y)) in ls.iter().zip(placed) {
        let title = display(&l.title);
        let t = match &l.number {
            Some(n) if !n.is_empty() => format!("{} {title}", display(n)),
            _ => title,
        };
        let mut v = serde_json::Map::new();
        v.insert("t".into(), json!(t));
        v.insert("p".into(), json!(p));
        v.insert("l".into(), json!(l.page));
        v.insert("y".into(), json!(y));
        let d = depth(&l.level);
        while stack.last().is_some_and(|n| n.d >= d) {
            let n = stack.pop().expect("a node");
            match stack.last_mut() {
                Some(up) => up.kids.push(n),
                None => top.push(close(n)),
            }
        }
        stack.push(Node {
            d,
            v,
            kids: Vec::new(),
        });
    }
    while let Some(n) = stack.pop() {
        match stack.last_mut() {
            Some(up) => up.kids.push(n),
            None => top.push(close(n)),
        }
    }
    Value::Array(top)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn from_the_aux_and_an_included_ones() {
        let aux = "\\relax \n\\@writefile{toc}{\\contentsline {section}{\\numberline {1}A}{1}{}\\protected@file@percent }\n\\@writefile{lof}{\\contentsline {figure}{F}{1}{}}\n\\@input{ch.aux}\n\\@writefile{toc}{\\contentsline {section}{\\numberline {3}C\nD}{5}{}}\n";
        let ch = "\\@writefile{toc}{\\contentsline {section}{\\numberline {2}B}{3}{}}\n";
        let toc = from_aux(aux, &|n| (n == "ch.aux").then(|| ch.to_owned()));
        let ts: Vec<String> = lines(&toc).into_iter().map(|l| l.title).collect();
        assert_eq!(ts, ["A", "B", "CD"]);
    }

    #[test]
    fn contentslines() {
        let toc = "\\contentsline {section}{\\numberline {1}Intro}{1}{}%\n\\contentsline {subsection}{\\numberline {1.1}The $\\alpha $ ca\nse}{2}{}%\n\\contentsline {section}{Index}{iv}{section*.3}%\n";
        let ls = lines(toc);
        assert_eq!(ls.len(), 3);
        assert_eq!(ls[0].number.as_deref(), Some("1"));
        assert_eq!(ls[1].title, "The $\\alpha $ case");
        assert_eq!((ls[2].number.as_deref(), ls[2].page.as_str()), (None, "iv"));
        assert_eq!(key(&ls[1].title), key("The $\\alpha$ case"));
        assert_eq!(
            display("\\LaTeX {} and~more --- \\emph {now}"),
            "LaTeX and more — now"
        );
    }

    /// Page origins of glyphs `(file, start, end, synth, y)`.
    fn page(gs: &[(u32, u32, u32, bool, f32)]) -> phitex_tex::PageOrigins {
        let mut o = phitex_tex::PageOrigins {
            synth: vec![0; gs.len().div_ceil(32)],
            ..Default::default()
        };
        for (i, &(f, s, e, syn, y)) in gs.iter().enumerate() {
            o.file.push(f);
            o.start.push(s);
            o.end.push(e);
            o.x.push(0.0);
            o.y.push(y);
            o.w.push(5.0);
            o.size.push(10.0);
            if syn {
                o.synth[i / 32] |= 1 << (i % 32);
            }
        }
        o
    }

    #[test]
    fn placed_on_the_headings_not_the_contents() {
        let src = "\\section{Intro}\nSee the Results.\n\\section{Results}\nText.\n";
        let at = |w: &str| u32::try_from(src.find(w).unwrap()).unwrap();
        let (intro, see, res) = (at("Intro"), at("See"), at("Results}"));
        let toc = "\\contentsline {section}{\\numberline {1}Intro}{1}{}%\n\\contentsline {section}{\\numberline {2}Results}{2}{}%\n";
        let run = |f: u32, s: u32, n: u32, y: f32| -> Vec<(u32, u32, u32, bool, f32)> {
            (0..n).map(|i| (f, s + i, s + i + 1, false, y)).collect()
        };
        // page 0: the contents (from the .toc), then section 1 and a
        // mention of "Results" in the text; page 1: section 2
        let mut p0 = run(1, 30, 5, 100.0);
        p0.push((0, 0, 15, true, 200.0));
        p0.extend(run(0, intro, 5, 200.0));
        p0.extend(run(0, see, 16, 220.0));
        let mut p1 = vec![(0, 32, 49, true, 50.0)];
        p1.extend(run(0, res, 7, 50.0));
        let snap = phitex_tex::OriginsSnapshot {
            pages: vec![Some(Arc::new(page(&p0))), Some(Arc::new(page(&p1)))],
            files: vec!["main.tex".into(), "main.toc".into()],
            texts: vec![
                Some(Arc::from(src.as_bytes())),
                Some(Arc::from(toc.as_bytes())),
            ],
        };
        let v = outline(toc, &snap, 2);
        assert_eq!(
            v,
            json!([
                {"t": "1 Intro", "p": 0, "l": "1", "y": 190.0, "k": []},
                {"t": "2 Results", "p": 1, "l": "2", "y": 40.0, "k": []},
            ])
        );
    }

    /// The contents' lines placed in the sources by their text, the
    /// titles' own bytes: the headings told by their printed numbers.
    #[test]
    fn contents_lines_set_from_the_titles() {
        let src = "\\tableofcontents\n\\section*{Preface}\nWords.\n\\section{Intro}\nText.\n\\section{Results}\nText.\n";
        let at = |w: &str| u32::try_from(src.find(w).unwrap()).unwrap();
        let toc = "\\contentsline {section}{Preface}{ii}{}%\n\\contentsline {section}{\\numberline {1}Intro}{1}{}%\n\\contentsline {section}{\\numberline {2}Results}{2}{}%\n";
        let run = |s: u32, n: u32, y: f32| -> Vec<(u32, u32, u32, bool, f32)> {
            (0..n).map(|i| (0, s + i, s + i + 1, false, y)).collect()
        };
        let synth = |y: f32| (0, 0, 0, true, y);
        // page 0 (i): the contents, its lines' glyphs from the titles;
        // page 1 (ii): the preface; pages 2 and 3 (1, 2): the sections
        let mut p0 = vec![];
        for (y, (w, n)) in [
            (100.0, ("Preface}", 7)),
            (120.0, ("Intro}", 5)),
            (140.0, ("Results}", 7)),
        ] {
            p0.push(synth(y));
            p0.extend(run(at(w), n, y));
            p0.push(synth(y));
        }
        let mut p1 = run(at("Preface}"), 7, 60.0);
        p1.extend(run(at("Words"), 6, 80.0));
        let mut p2 = vec![synth(70.0)];
        p2.extend(run(at("Intro}"), 5, 70.0));
        let mut p3 = vec![synth(90.0)];
        p3.extend(run(at("Results}"), 7, 90.0));
        let snap = phitex_tex::OriginsSnapshot {
            pages: [p0, p1, p2, p3]
                .iter()
                .map(|p| Some(Arc::new(page(p))))
                .collect(),
            files: vec!["main.tex".into()],
            texts: vec![Some(Arc::from(src.as_bytes()))],
        };
        let at: Vec<(Option<usize>, Option<f64>)> = place(&lines(toc), &snap, 4);
        assert_eq!(
            at,
            [
                (Some(1), Some(50.0)),
                (Some(2), Some(60.0)),
                (Some(3), Some(80.0))
            ]
        );
    }

    #[test]
    fn nested_and_unplaced_by_their_numbers() {
        let toc = "\\contentsline {section}{\\numberline {1}A}{3}{}%\n\\contentsline {subsection}{\\numberline {1.1}B}{4}{}%\n\\contentsline {section}{\\numberline {2}C}{9}{}%\n";
        let src = "\\section{A}\n";
        let snap = phitex_tex::OriginsSnapshot {
            pages: vec![None, Some(Arc::new(page(&[(0, 9, 10, false, 80.0)])))],
            files: vec!["main.tex".into()],
            texts: vec![Some(Arc::from(src.as_bytes()))],
        };
        let v = outline(toc, &snap, 5);
        assert_eq!(
            v,
            json!([
                {"t": "1 A", "p": 1, "l": "3", "y": 70.0, "k": [
                    {"t": "1.1 B", "p": 2, "l": "4", "y": null, "k": []},
                ]},
                // (page 9 is past the PDF's 5)
                {"t": "2 C", "p": null, "l": "9", "y": null, "k": []},
            ])
        );
    }
}
