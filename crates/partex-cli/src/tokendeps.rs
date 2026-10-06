//! Source text as values (DESIGN.md §7.2): what a line of an input file
//! defines is the tokens TeX reads from it, not its bytes. A line read
//! whole under one set of category codes, never shown in an error
//! context, defines the same tokens after an edit if it reads the same
//! under those codes (`line_tokens`), and then nothing TeX does can tell
//! the edit apart: when every changed line of every changed file is so,
//! a rebuild runs nothing. `PARTEX_TOKEN_DEPS=0` turns it off.
//!
//! Every test here refuses when in doubt: a refusal costs an ordinary
//! rebuild, a wrong acceptance a wrong document.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use partex_core::track::{file_lines, line_tokens};

use crate::intervals::{Interval, IntervalReads, LineEvent, file_id};

/// A changed line longer than this is always visible: TeX's buffer is
/// shared by every open line and `\csname`, and a longer line could
/// overflow it where the old one did not.
const LONGEST_LINE: usize = 4096;

/// Whether token-level input dependencies are on.
pub fn enabled() -> bool {
    std::env::var_os("PARTEX_TOKEN_DEPS").is_none_or(|v| v != "0")
}

/// Where lines of `old` begin in `new`, and whether each changed: with as
/// many lines, every line (and the end to the end); otherwise only the
/// lines of the common beginning and end, unchanged (the others are
/// missing from the map).
pub fn line_map(old: &[u8], new: &[u8]) -> HashMap<usize, (usize, bool)> {
    let (a, b) = (file_lines(old), file_lines(new));
    let mut m: HashMap<usize, (usize, bool)> = HashMap::new();
    m.insert(old.len(), (new.len(), false));
    if a.len() == b.len() {
        m.extend(
            a.iter()
                .zip(&b)
                .map(|(x, y)| (x.0, (y.0, old[x.0..x.1] != new[y.0..y.1]))),
        );
        return m;
    }
    // (a whole line, with its end: the same text now ends another way)
    let same =
        |x: &(usize, usize, usize), y: &(usize, usize, usize)| old[x.0..x.2] == new[y.0..y.2];
    let head = a.iter().zip(&b).take_while(|(x, y)| same(x, y)).count();
    let tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(a.len().min(b.len()) - head)
        .take_while(|(x, y)| same(x, y))
        .count();
    m.extend(a[..head].iter().zip(&b).map(|(x, y)| (x.0, (y.0, false))));
    m.extend(
        a[a.len() - tail..]
            .iter()
            .zip(&b[b.len() - tail..])
            .map(|(x, y)| (x.0, (y.0, false))),
    );
    m
}

/// The events about `old` are about `new` now: its lines by where they
/// begin in it, and those of lines no longer there marked unusable.
pub fn remap(
    intervals: &mut [(u64, Interval)],
    tracker: &IntervalReads,
    old: &Arc<[u8]>,
    new: &Arc<[u8]>,
) {
    let (o, n) = (file_id(old), file_id(new));
    if o == n
        || !intervals
            .iter()
            .any(|(_, iv)| iv.lines.iter().any(|e| e.file() == o))
    {
        return;
    }
    tracker.pin(new);
    let map = line_map(old, new);
    let at = |f: usize| map.get(&f).map(|m| m.0);
    for (_, iv) in intervals {
        for e in &mut iv.lines {
            if e.file() != o {
                continue;
            }
            *e = match *e {
                LineEvent::Open(_) => LineEvent::Open(n),
                LineEvent::Poison(_) => LineEvent::Poison(n),
                LineEvent::Start(_, f) => {
                    at(f).map_or(LineEvent::Poison(n), |g| LineEvent::Start(n, g))
                }
                LineEvent::End(_, f, c) => {
                    at(f).map_or(LineEvent::Poison(n), |g| LineEvent::End(n, g, c))
                }
                LineEvent::Shown(_, f) => {
                    at(f).map_or(LineEvent::Poison(n), |g| LineEvent::Shown(n, g))
                }
            };
        }
    }
}

/// Whether `intervals` record the whole job of `total` commands, one after
/// another from the start, every one recorded: else a line may have been
/// read where no event tells.
pub fn covers(intervals: &[(u64, Interval)], total: u64) -> Result<(), String> {
    let mut at = 0;
    for (from, iv) in intervals {
        if !iv.recorded {
            return Err("an interval was not recorded".to_owned());
        }
        if *from != at {
            return Err(format!(
                "the intervals recorded leave out commands {at}..{from}"
            ));
        }
        at = iv.to;
    }
    if at == total {
        Ok(())
    } else {
        Err(format!(
            "the intervals recorded stop at command {at} of {total}"
        ))
    }
}

/// Whether `old`, found by `lookups` lookups of the build `intervals`
/// record, can become `new` unseen by what read it by lines: the number
/// of lines that changed, and whether some lookups were not read by lines
/// (`\pdffilesize` and the like: the caller runs the intervals they are
/// in again to see), or why not (`name` the file's).
pub fn invisible(
    intervals: &[(u64, Interval)],
    tracker: &IntervalReads,
    old: &[u8],
    new: &[u8],
    lookups: usize,
    name: &str,
) -> Result<(usize, bool), String> {
    let id = file_id(old);
    let mut opens = 0;
    let mut starts: HashMap<usize, u32> = HashMap::new();
    let mut ends: HashMap<usize, (u32, Vec<u32>)> = HashMap::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for e in intervals.iter().flat_map(|(_, iv)| &iv.lines) {
        if e.file() != id {
            continue;
        }
        match *e {
            LineEvent::Open(_) => opens += 1,
            LineEvent::Start(_, f) => *starts.entry(f).or_default() += 1,
            LineEvent::End(_, f, Some(c)) => {
                let (n, codes) = ends.entry(f).or_default();
                *n += 1;
                if !codes.contains(&c) {
                    codes.push(c);
                }
            }
            LineEvent::End(_, f, None) | LineEvent::Shown(_, f) => {
                seen.insert(f);
            }
            LineEvent::Poison(_) => {
                return Err(format!("{name}: lines were added or removed before"));
            }
        }
    }
    if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
        let all: usize = intervals.iter().map(|(_, iv)| iv.lines.len()).sum();
        eprintln!(
            "phitex: {name}: {all} line events over {} intervals; {} opens here, {} starts",
            intervals.len(),
            opens,
            starts.len()
        );
    }
    if opens > lookups {
        return Err(format!("{name}: opened more often than looked up"));
    }
    let (was_lines, is_lines) = (file_lines(old), file_lines(new));
    if was_lines.len() != is_lines.len() {
        return Err(format!(
            "{name}: {} lines, now {}",
            was_lines.len(),
            is_lines.len()
        ));
    }
    let mut changed = 0;
    for (k, (x, y)) in was_lines.iter().zip(&is_lines).enumerate() {
        let (was, is) = (&old[x.0..x.1], &new[y.0..y.1]);
        if was == is {
            continue;
        }
        changed += 1;
        let line = k + 1;
        if is.len() > LONGEST_LINE {
            return Err(format!("{name}:{line} is long"));
        }
        if seen.contains(&x.0) {
            return Err(format!(
                "{name}:{line} was shown in an error, or read while category codes changed"
            ));
        }
        let (ended, codes) = ends.get(&x.0).cloned().unwrap_or_default();
        if starts.get(&x.0).copied().unwrap_or(0) != ended {
            return Err(format!(
                "{name}:{line} was not read whole as a line of \\input"
            ));
        }
        for c in codes {
            let codes = tracker
                .codes(c)
                .ok_or_else(|| format!("{name}:{line}: codes lost"))?;
            match (line_tokens(was, &codes), line_tokens(is, &codes)) {
                (Some(then), Some(now)) if then == now => {}
                _ => return Err(format!("{name}:{line} reads as other tokens")),
            }
        }
    }
    Ok((changed, opens != lookups))
}

#[cfg(test)]
mod tests {
    use super::*;
    use partex_core::track::{LineCodes, LineToken, Tracker};

    fn plain() -> LineCodes {
        let mut cat = [12u8; 256];
        for c in (b'a'..=b'z').chain(b'A'..=b'Z') {
            cat[usize::from(c)] = 11;
        }
        cat[usize::from(b'\\')] = 0;
        cat[usize::from(b'{')] = 1;
        cat[usize::from(b'}')] = 2;
        cat[usize::from(b'$')] = 3;
        cat[usize::from(b'&')] = 4;
        cat[usize::from(b'#')] = 6;
        cat[usize::from(b'^')] = 7;
        cat[usize::from(b'_')] = 8;
        cat[0] = 9;
        cat[usize::from(b' ')] = 10;
        cat[usize::from(b'\t')] = 10;
        cat[usize::from(b'%')] = 14;
        cat[usize::from(b'\r')] = 5;
        cat[usize::from(b'~')] = 13;
        cat[0x7f] = 15;
        LineCodes {
            cat,
            wide: Vec::new(),
            end_line_char: 13,
        }
    }

    #[test]
    fn same_tokens() {
        let c = plain();
        let t = |s: &[u8]| line_tokens(s, &c);
        assert_eq!(t(b"a  b"), t(b"a b"));
        assert_eq!(t(b"a\tb"), t(b"a b"));
        assert_eq!(t(b"  a b"), t(b"a b"));
        assert_eq!(t(b"\\foo   x"), t(b"\\foo x"));
        assert_eq!(t(b"x% one"), t(b"x% two"));
        assert_eq!(t(b"x%"), t(b"x%   comment"));
        assert_ne!(t(b"x%"), t(b"x"));
        assert_ne!(t(b"\\foo x"), t(b"\\foox"));
        assert_ne!(t(b"a b"), t(b"ab"));
        assert_ne!(t(b"a"), t(b"b"));
        assert_ne!(t(b"{a}"), t(b"a"));
        // a line's end is a space, but not after a control word
        assert_eq!(t(b"a"), t(b"a "));
        assert_eq!(t(b"\\relax"), t(b"\\relax  "));
        assert_eq!(t(b"\\relax"), t(b"\\relax%"));
        assert_ne!(t(b"a"), t(b"a%"));
        assert_eq!(t(b""), Some(vec![LineToken::Par]));
        assert_eq!(t(b"   "), t(b""));
        assert_ne!(t(b"%"), t(b""));
        // `^^` notation and invalid characters: TeX rewrites the buffer
        // or stops, so never the same
        assert_eq!(t(b"x^^41"), None);
        assert_eq!(t(b"\\a^^41"), None);
        assert_eq!(t(b"\x7f"), None);
        assert_ne!(t(b"x^y"), None);
        // an ignored character
        assert_eq!(t(b"a\x00b"), t(b"ab"));
        assert_eq!(
            t(b"\\ ~"),
            Some(vec![
                LineToken::Cs(vec![u32::from(b' ')]),
                LineToken::Active(u32::from(b'~')),
                LineToken::Char(10, u32::from(b' '))
            ])
        );
        // a backslash at the end names `\^^M`
        assert_eq!(
            t(b"a\\"),
            Some(vec![
                LineToken::Char(11, u32::from(b'a')),
                LineToken::Cs(vec![13])
            ])
        );
    }

    #[test]
    fn tokens_follow_the_codes() {
        let c = plain();
        // \obeyspaces: every space is a token
        let mut obey = plain();
        obey.cat[usize::from(b' ')] = 13;
        assert_eq!(line_tokens(b"a  b", &c), line_tokens(b"a b", &c));
        assert_ne!(line_tokens(b"a  b", &obey), line_tokens(b"a b", &obey));
        // \makeatletter
        let mut at = plain();
        at.cat[usize::from(b'@')] = 11;
        assert_eq!(
            line_tokens(b"\\a@b x", &c).unwrap().len(),
            6 // \a @ b space x, and the end of the line
        );
        assert_eq!(line_tokens(b"\\a@b  x", &at), line_tokens(b"\\a@b x", &at));
        assert_eq!(line_tokens(b"\\a@b x", &at).unwrap().len(), 3);
        // \endlinechar=-1: no space at the end
        let mut no_end = plain();
        no_end.end_line_char = -1;
        assert_eq!(line_tokens(b"a", &no_end).unwrap().len(), 1);
        assert_eq!(line_tokens(b"", &no_end), Some(vec![]));
        assert_ne!(line_tokens(b"", &no_end), line_tokens(b"", &c));
    }

    #[test]
    fn maps_lines() {
        let m = line_map(b"a\nb  b\nc\n", b"a\nb b\nc\n");
        assert_eq!(m[&0], (0, false));
        assert_eq!(m[&2], (2, true));
        assert_eq!(m[&7], (6, false));
        assert_eq!(m[&9], (8, false));
        // a line added: the lines before and after it move
        let m = line_map(b"a\nb\nc\n", b"a\nx\nb\nc\n");
        assert_eq!(m[&0], (0, false));
        assert_eq!(m[&2], (4, false));
        assert_eq!(m[&4], (6, false));
        assert_eq!(m[&6], (8, false));
        // a line changed and one removed: that line is gone
        let m = line_map(b"a\nb\nc\nd\n", b"a\nB\nd\n");
        assert_eq!(m.get(&2), None);
        assert_eq!(m.get(&4), None);
        assert_eq!(m[&6], (4, false));
        // the same line text, but now the last one (no end of line)
        let m = line_map(b"a\nb\n", b"a\nb");
        assert_eq!(m[&2], (2, false));
        let m = line_map(b"a\nb\n", b"x\na\nb");
        assert_eq!(m.get(&2), None);
    }

    fn file(s: &[u8]) -> Arc<[u8]> {
        Arc::from(s)
    }

    /// A tracker, and the events of reading `f` line by line under the
    /// codes `plain` gives (as the core reports them).
    fn read(f: &Arc<[u8]>, shown: Option<usize>) -> (IntervalReads, Interval) {
        let t = IntervalReads::default();
        t.lines_open(f);
        for (from, _, _) in file_lines(f) {
            let g = t.line_start(f, from);
            if shown == Some(from) {
                t.line_shown(f, from);
            }
            t.line_end(f, from, g, || Some(plain()));
        }
        let iv = t.take();
        (t, iv)
    }

    #[test]
    fn invisible_edits() {
        if !enabled() {
            return;
        }
        let old = file(b"\\def\\x{a  b}\n% a comment\ntext  here\n\\x\n");
        let (t, iv) = read(&old, None);
        let ivs = vec![(0, iv)];
        let check = |new: &[u8]| invisible(&ivs, &t, &old, new, 1, "f.tex");
        assert_eq!(
            check(b"\\def\\x{a b}\n% another comment\ntext here\n\\x\n"),
            Ok((3, false))
        );
        assert_eq!(check(&old), Ok((0, false)));
        assert!(check(b"\\def\\x{a b}\n% a comment\ntext there\n\\x\n").is_err());
        // a line more, or fewer
        assert!(check(b"\\def\\x{a b}\n% a comment\ntext here\n\n\\x\n").is_err());
        assert!(check(b"\\def\\x{a b}\ntext here\n\\x\n").is_err());
        // `^^` notation
        assert!(check(b"\\def\\x{a b}\n% a comment\ntext^^20here\n\\x\n").is_err());
        // a lookup that did not read by lines: the caller must see to it
        assert_eq!(
            invisible(
                &ivs,
                &t,
                &old,
                b"\\def\\x{a b}\n% a comment\ntext here\n\\x\n",
                2,
                "f"
            ),
            Ok((2, true))
        );
    }

    #[test]
    fn visible_edits() {
        if !enabled() {
            return;
        }
        let old = file(b"a\nb  c\nd\n");
        let new: &[u8] = b"a\nb c\nd\n";
        // shown in an error context
        let (t, iv) = read(&old, Some(2));
        assert!(invisible(&[(0, iv)], &t, &old, new, 1, "f").is_err());
        // read while the codes changed
        let t = IntervalReads::default();
        t.lines_open(&old);
        let g = t.line_start(&old, 2);
        let catcode = (0..1 << 20)
            .find(|&p| partex_core::track::tokenizes(p))
            .expect("a category code");
        t.write(partex_core::track::Cell::Eqtb(catcode));
        t.line_end(&old, 2, g, || Some(plain()));
        assert!(invisible(&[(0, t.take())], &t, &old, new, 1, "f").is_err());
        // begun but never ended (a `\read` line, or the job stopped in it)
        let t = IntervalReads::default();
        t.lines_open(&old);
        t.line_start(&old, 2);
        assert!(invisible(&[(0, t.take())], &t, &old, new, 1, "f").is_err());
        // read under \obeyspaces
        let t = IntervalReads::default();
        t.lines_open(&old);
        let g = t.line_start(&old, 2);
        let mut obey = plain();
        obey.cat[usize::from(b' ')] = 13;
        t.line_end(&old, 2, g, || Some(obey.clone()));
        assert!(invisible(&[(0, t.take())], &t, &old, new, 1, "f").is_err());
        // two readings, one under each set of codes
        let (t, iv) = read(&old, None);
        let g = t.line_start(&old, 2);
        t.line_end(&old, 2, g, || Some(obey));
        let ivs = vec![(0, iv), (10, t.take())];
        assert!(invisible(&ivs, &t, &old, new, 2, "f").is_err());
        // with the codes not known (input translation): never the same
        let t = IntervalReads::default();
        t.lines_open(&old);
        let g = t.line_start(&old, 2);
        t.line_end(&old, 2, g, || None);
        assert!(invisible(&[(0, t.take())], &t, &old, new, 1, "f").is_err());
    }

    #[test]
    fn events_move_with_the_file() {
        if !enabled() {
            return;
        }
        let old = file(b"a\nb  c\nd\n");
        let (t, iv) = read(&old, None);
        let mut ivs = vec![(0, iv)];
        let new = file(b"a\nb c\nd\n");
        remap(&mut ivs, &t, &old, &new);
        assert!(ivs[0].1.lines.iter().all(|e| e.file() == file_id(&new)));
        assert!(t.pinned(file_id(&new)).is_some());
        let newer: &[u8] = b"a\nb    c\nd\n";
        assert_eq!(invisible(&ivs, &t, &new, newer, 1, "f"), Ok((1, false)));
        // lines added: the events of lines no longer there are unusable
        let added = file(b"a\nx\nb c\nd\n");
        remap(&mut ivs, &t, &new, &added);
        assert!(
            !ivs[0]
                .1
                .lines
                .iter()
                .any(|e| matches!(e, LineEvent::Poison(_)))
        );
        let changed = file(b"a\nB\nd\n");
        remap(&mut ivs, &t, &added, &changed);
        assert!(
            ivs[0]
                .1
                .lines
                .iter()
                .any(|e| matches!(e, LineEvent::Poison(_)))
        );
        assert!(invisible(&ivs, &t, &changed, b"a\nB \nd\n", 1, "f").is_err());
        // no events name the old files now
        t.prune(&ivs);
        assert_eq!(t.pins().len(), 1);
    }

    #[test]
    fn coverage() {
        let iv = |to| Interval {
            to,
            recorded: true,
            ..Interval::default()
        };
        assert!(covers(&[(0, iv(10)), (10, iv(25))], 25).is_ok());
        assert!(covers(&[(0, iv(10)), (12, iv(25))], 25).is_err());
        assert!(covers(&[(5, iv(10)), (10, iv(25))], 25).is_err());
        assert!(covers(&[(0, iv(10)), (10, iv(20))], 25).is_err());
        assert!(covers(&[], 0).is_ok());
        assert!(covers(&[(0, Interval::default())], 0).is_err());
    }
}
