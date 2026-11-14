//! The texts under review (docs/spec-editor-protocol.md §9): two versions
//! of a file compared by words, the hunks a reviewer accepts or rejects,
//! and places carried from one version to the next (a note's anchor, the
//! ranges an agent edited). Pure: no files, no watch.
//!
//! - [`changes`]: the changed runs of two texts, by tokens (a word, a run
//!   of spaces, any other character alone), the unchanged prefix and
//!   suffix first, the middle by patience diff ([`phitex_diff::myers`]).
//! - [`hunks`]: the changes joined where fewer than [`JOIN_WORDS`] words
//!   on one paragraph lie between them, each with its words marked.
//! - [`map_range`]: a byte range of the old text in the new one, through
//!   the changes; [`find_quote`]: a quote looked for by its context.

use phitex_diff::myers::patience;

/// Changes closer than this many words, on one paragraph, are one hunk.
pub const JOIN_WORDS: usize = 3;

/// The context a note's anchor keeps on each side, in bytes at most.
pub const CONTEXT: usize = 64;

/// One changed run: bytes `a` of the old text became bytes `b` of the new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub a: (usize, usize),
    pub b: (usize, usize),
}

/// Whether `c` is part of a word: letters, digits, `'`, `-` and any
/// character past ASCII (the viewer's and the cursor's rule, §3.1).
fn wordy(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '-' || !c.is_ascii()
}

/// The tokens of `s` as byte ranges: words, runs of white space, and
/// each other character alone (a backslash, a brace, a dollar).
pub fn tokens(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let mut j = i + c.len_utf8();
        let same = |d: char| {
            if wordy(c) {
                wordy(d)
            } else if c.is_whitespace() {
                d.is_whitespace()
            } else {
                false
            }
        };
        while let Some(&(k, d)) = it.peek() {
            if !same(d) {
                break;
            }
            j = k + d.len_utf8();
            it.next();
        }
        out.push((i, j));
    }
    out
}

/// Whether byte `i` of `s` is inside a word (its character and the one
/// before it both word characters): a cut there splits the word.
fn mid_word(s: &str, i: usize) -> bool {
    let before = s[..i].chars().next_back();
    let after = s[i..].chars().next();
    matches!((before, after), (Some(x), Some(y)) if wordy(x) && wordy(y))
}

/// The changed runs from `a` to `b`, in order: the common prefix and
/// suffix (cut back to whole words) left out, the rest diffed by tokens.
pub fn changes(a: &str, b: &str) -> Vec<Change> {
    if a == b {
        return Vec::new();
    }
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let mut p = ab.iter().zip(bb).take_while(|(x, y)| x == y).count();
    while p > 0
        && (!a.is_char_boundary(p) || !b.is_char_boundary(p) || mid_word(a, p) || mid_word(b, p))
    {
        p -= 1;
    }
    let max_s = (ab.len() - p).min(bb.len() - p);
    let mut s = ab
        .iter()
        .rev()
        .zip(bb.iter().rev())
        .take(max_s)
        .take_while(|(x, y)| x == y)
        .count();
    while s > 0
        && (!a.is_char_boundary(ab.len() - s)
            || !b.is_char_boundary(bb.len() - s)
            || mid_word(a, ab.len() - s)
            || mid_word(b, bb.len() - s))
    {
        s -= 1;
    }
    let (am, bm) = (&a[p..ab.len() - s], &b[p..bb.len() - s]);
    let (ta, tb) = (tokens(am), tokens(bm));
    let wa: Vec<&str> = ta.iter().map(|&(x, y)| &am[x..y]).collect();
    let wb: Vec<&str> = tb.iter().map(|&(x, y)| &bm[x..y]).collect();
    let pairs = patience(&wa, &wb);
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let at_a = |k: usize| if k < ta.len() { ta[k].0 } else { am.len() };
    let at_b = |k: usize| if k < tb.len() { tb[k].0 } else { bm.len() };
    for &(x, y) in pairs.iter().chain(std::iter::once(&(ta.len(), tb.len()))) {
        if x > i || y > j {
            out.push(Change {
                a: (p + at_a(i), p + at_a(x)),
                b: (p + at_b(j), p + at_b(y)),
            });
        }
        i = x + 1;
        j = y + 1;
    }
    slide(a, b, &mut out);
    out
}

/// Each insertion and deletion that begins with white space and could
/// as well begin after it (`with| many| words` as `with |many |words`)
/// moved there: one change is said one way, whatever text around it was
/// diffed with it (a hunk's name is its text, `review.rs`).
fn slide(a: &str, b: &str, cs: &mut [Change]) {
    // (the first character of `t[x0..x1]` if it is white space and the
    // text after it begins with it too)
    let first = |t: &str, x0: usize, x1: usize| {
        let c = t[x0..x1].chars().next().filter(|c| c.is_whitespace())?;
        t[x1..].starts_with(c).then_some(c.len_utf8())
    };
    for k in 0..cs.len() {
        let (limit_a, limit_b) = cs.get(k + 1).map_or((a.len(), b.len()), |n| (n.a.0, n.b.0));
        let c = &mut cs[k];
        loop {
            let n = if c.a.0 == c.a.1 && c.b.1 > c.b.0 {
                first(b, c.b.0, c.b.1)
            } else if c.b.0 == c.b.1 && c.a.1 > c.a.0 {
                first(a, c.a.0, c.a.1)
            } else {
                None
            };
            // (moved only over text both have, before the next change)
            let Some(n) = n.filter(|n| c.a.1 + n <= limit_a && c.b.1 + n <= limit_b) else {
                break;
            };
            c.a = (c.a.0 + n, c.a.1 + n);
            c.b = (c.b.0 + n, c.b.1 + n);
        }
    }
}

/// A part of a hunk as a reader sees it: the same in both, removed, added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Same,
    Del,
    Add,
}

/// A hunk: one place the new text differs from the old, its words marked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// Its bytes in the old text and in the new.
    pub a: (usize, usize),
    pub b: (usize, usize),
    /// Its text, part by part, from its first change to its last.
    pub parts: Vec<(Part, String)>,
}

impl Hunk {
    /// Its old text and its new one.
    pub fn old_new(&self) -> (String, String) {
        let mut o = String::new();
        let mut n = String::new();
        for (k, t) in &self.parts {
            if *k != Part::Add {
                o.push_str(t);
            }
            if *k != Part::Del {
                n.push_str(t);
            }
        }
        (o, n)
    }
}

/// The words of `s` (tokens with a word character).
fn words(s: &str) -> usize {
    tokens(s)
        .iter()
        .filter(|&&(x, y)| s[x..y].chars().any(wordy))
        .count()
}

/// The hunks from `a` to `b`: [`changes`] joined where the text between two
/// is fewer than [`JOIN_WORDS`] words and no blank line.
pub fn hunks(a: &str, b: &str) -> Vec<Hunk> {
    let cs = changes(a, b);
    let mut out: Vec<Hunk> = Vec::new();
    for c in cs {
        if let Some(h) = out.last_mut() {
            let gap = &b[h.b.1..c.b.0];
            if words(gap) < JOIN_WORDS && !gap.contains("\n\n") && !gap.contains("\n\r\n") {
                if !gap.is_empty() {
                    h.parts.push((Part::Same, gap.to_owned()));
                }
                push_change(&mut h.parts, a, b, c);
                h.a.1 = c.a.1;
                h.b.1 = c.b.1;
                continue;
            }
        }
        let mut parts = Vec::new();
        push_change(&mut parts, a, b, c);
        out.push(Hunk {
            a: c.a,
            b: c.b,
            parts,
        });
    }
    for h in &mut out {
        dense(h);
    }
    out
}

/// A hunk mostly changed (a sentence rewritten) said as its old text, then
/// its new: word by word, `The quickA brownheron` reads as noise. Where
/// the words kept are fewer than half those changed.
fn dense(h: &mut Hunk) {
    let count = |same: bool| -> usize {
        h.parts
            .iter()
            .filter(|(k, _)| (*k == Part::Same) == same)
            .map(|(_, t)| words(t))
            .sum()
    };
    let gaps = h.parts.iter().filter(|(k, _)| *k == Part::Same).count();
    if gaps < 2 || count(true) * 2 >= count(false) {
        return;
    }
    let (o, n) = h.old_new();
    h.parts = [(Part::Del, o), (Part::Add, n)]
        .into_iter()
        .filter(|(_, t)| !t.is_empty())
        .collect();
}

fn push_change(parts: &mut Vec<(Part, String)>, a: &str, b: &str, c: Change) {
    if c.a.1 > c.a.0 {
        parts.push((Part::Del, a[c.a.0..c.a.1].to_owned()));
    }
    if c.b.1 > c.b.0 {
        parts.push((Part::Add, b[c.b.0..c.b.1].to_owned()));
    }
}

/// Where byte range `r` of `a` is in `b`, through `cs` ([`changes`] from
/// `a` to `b`): a change inside it is taken in; its start in a change
/// moves to that change's start in `b`, its end to the change's end; text
/// inserted right at its start or end is left out. None: every token of
/// it was changed (its text is gone from here).
pub fn map_range(cs: &[Change], r: (usize, usize)) -> Option<(usize, usize)> {
    let (s, e) = r;
    let mut delta: isize = 0;
    let mut ns = None;
    let mut ne = None;
    for c in cs {
        // (the whole range inside one change, or the change covers it: gone)
        if c.a.0 <= s && e <= c.a.1 && c.a.1 > c.a.0 && !(s == e && (s == c.a.0 || s == c.a.1)) {
            return None;
        }
        if ns.is_none() {
            if c.a.0 > s {
                ns = Some(shift(s, delta));
            } else if c.a.0 == s && c.a.1 == s {
                // (inserted right at its start: after it)
                ns = Some(c.b.1);
            } else if c.a.0 <= s && s < c.a.1 {
                ns = Some(c.b.0);
            }
        }
        if ne.is_none() {
            if c.a.0 >= e {
                ne = Some(shift(e, delta));
            } else if c.a.0 < e && e <= c.a.1 {
                ne = Some(c.b.1);
            }
        }
        delta += growth(c.a, c.b);
    }
    let ns = ns.unwrap_or_else(|| shift(s, delta));
    let ne = ne.unwrap_or_else(|| shift(e, delta)).max(ns);
    Some((ns, ne))
}

/// How much longer `b` is than `a` (byte ranges of a text: never past
/// `isize`'s range).
pub fn growth(a: (usize, usize), b: (usize, usize)) -> isize {
    let len = |r: (usize, usize)| isize::try_from(r.1 - r.0).unwrap_or(isize::MAX);
    len(b) - len(a)
}

fn shift(x: usize, d: isize) -> usize {
    x.saturating_add_signed(d)
}

/// Byte `x` of `a` in `b` ([`map_range`] of an empty range; a byte in a
/// changed run goes to that change's start).
pub fn map_at(cs: &[Change], x: usize) -> usize {
    let mut delta: isize = 0;
    for c in cs {
        if c.a.0 >= x {
            break;
        }
        if x < c.a.1 {
            return c.b.0;
        }
        delta += growth(c.a, c.b);
    }
    shift(x, delta)
}

/// Where `quote` is in `text`, best by its context: the occurrence whose
/// bytes before it end as `before` does and after it begin as `after`
/// does, the longest such match first, then the nearest to `near`. A quote
/// of fewer than four bytes needs some context. None: nowhere.
pub fn find_quote(
    text: &str,
    quote: &str,
    before: &str,
    after: &str,
    near: usize,
) -> Option<(usize, usize)> {
    if quote.is_empty() {
        return None;
    }
    let mut best: Option<(usize, usize, usize)> = None;
    let mut from = 0;
    while let Some(k) = text[from..].find(quote) {
        let at = from + k;
        let pre = text[..at]
            .bytes()
            .rev()
            .zip(before.bytes().rev())
            .take_while(|(x, y)| x == y)
            .count();
        let post = text[at + quote.len()..]
            .bytes()
            .zip(after.bytes())
            .take_while(|(x, y)| x == y)
            .count();
        let score = pre + post;
        let dist = at.abs_diff(near);
        if quote.len() >= 4 || score > 0 {
            let better = match best {
                None => true,
                Some((s, d, _)) => score > s || (score == s && dist < d),
            };
            if better {
                best = Some((score, dist, at));
            }
        }
        from = at + quote.chars().next().map_or(1, char::len_utf8);
    }
    best.map(|(_, _, at)| (at, at + quote.len()))
}

/// The letters and digits of `s`, lowercased, each with its byte in `s`
/// (a page's ligatures as their letters): text compared as a reader sees
/// it, whatever the markup, spaces and punctuation around it.
pub fn letters(s: &str) -> Vec<(char, usize)> {
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        let lig = match c {
            '\u{fb00}' => "ff",
            '\u{fb01}' => "fi",
            '\u{fb02}' => "fl",
            '\u{fb03}' => "ffi",
            '\u{fb04}' => "ffl",
            _ => "",
        };
        if !lig.is_empty() {
            out.extend(lig.chars().map(|l| (l, i)));
        } else if c.is_alphanumeric() {
            out.extend(c.to_lowercase().map(|l| (l, i)));
        }
    }
    out
}

/// Where the words `seen` on a page are in source `text`, by their letters
/// ([`letters`]), the match nearest `near`: from the first letter's byte to
/// the end of the last's character. None: nowhere, or fewer than four letters.
pub fn find_seen(text: &str, seen: &str, near: usize) -> Option<(usize, usize)> {
    let want: Vec<char> = letters(seen).into_iter().map(|x| x.0).collect();
    if want.len() < 4 {
        return None;
    }
    let have = letters(text);
    let chars: Vec<char> = have.iter().map(|x| x.0).collect();
    let mut best: Option<(usize, usize, usize)> = None;
    for i in 0..chars.len().saturating_sub(want.len() - 1) {
        if chars[i..i + want.len()] == want[..] {
            let s = have[i].1;
            let last = have[i + want.len() - 1].1;
            let e = last + text[last..].chars().next().map_or(1, char::len_utf8);
            let d = s.abs_diff(near);
            if best.is_none_or(|b| d < b.2) {
                best = Some((s, e, d));
            }
        }
    }
    best.map(|(s, e, _)| (s, e))
}

/// Whether the source `quote` shows the words `seen` (their letters in it).
pub fn shows(quote: &str, seen: &str) -> bool {
    let q: String = letters(quote).into_iter().map(|x| x.0).collect();
    let w: String = letters(seen).into_iter().map(|x| x.0).collect();
    w.is_empty() || q.contains(&w)
}

/// The line (from 1) of byte `x` of `text`.
#[allow(
    clippy::naive_bytecount,
    reason = "a line number when asked, not a hot loop"
)]
pub fn line_of(text: &str, x: usize) -> usize {
    let x = x.min(text.len());
    text.as_bytes()[..x].iter().filter(|&&c| c == b'\n').count() + 1
}

/// `x` moved back to a character boundary of `text`.
pub fn floor(text: &str, mut x: usize) -> usize {
    x = x.min(text.len());
    while !text.is_char_boundary(x) {
        x -= 1;
    }
    x
}

/// The context of range `r` of `text`: up to [`CONTEXT`] bytes before and
/// after it, whole characters.
pub fn context(text: &str, r: (usize, usize)) -> (String, String) {
    let s = floor(text, r.0);
    let e = floor(text, r.1);
    let b0 = floor(text, s.saturating_sub(CONTEXT));
    let mut a1 = (e + CONTEXT).min(text.len());
    while !text.is_char_boundary(a1) {
        a1 += 1;
    }
    (text[b0..s].to_owned(), text[e..a1].to_owned())
}

/// The ranges of `now` that differ from `then` ([`changes`]), joined where
/// they touch; each with its first and last line.
pub fn edited(then: &str, now: &str) -> Vec<(usize, usize, usize, usize)> {
    hunks(then, now)
        .into_iter()
        .map(|h| {
            let last = if h.b.1 > h.b.0 { h.b.1 - 1 } else { h.b.0 };
            (h.b.0, h.b.1, line_of(now, h.b.0), line_of(now, last))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(a: &str, b: &str) -> String {
        // (the hunks applied to `a` give `b`)
        let hs = hunks(a, b);
        let mut out = a.to_owned();
        for h in hs.iter().rev() {
            let (_, n) = h.old_new();
            out.replace_range(h.a.0..h.a.1, &n);
        }
        out
    }

    #[test]
    fn tokens_are_words_spaces_and_single_signs() {
        let s = "Hello,  wörld \\emph{x}";
        let t: Vec<&str> = tokens(s).iter().map(|&(a, b)| &s[a..b]).collect();
        assert_eq!(
            t,
            [
                "Hello", ",", "  ", "wörld", " ", "\\", "emph", "{", "x", "}"
            ]
        );
    }

    #[test]
    fn a_sentence_rewritten_is_said_old_then_new() {
        let a = "The quick brown fox jumps over the lazy dog. Rest.";
        let b = "A heron stood motionless in the shallows at dawn. Rest.";
        let hs = hunks(a, b);
        assert_eq!(hs.len(), 1);
        assert_eq!(
            hs[0].parts.iter().map(|p| p.0).collect::<Vec<_>>(),
            [Part::Del, Part::Add]
        );
        // (a few words changed among many kept: word by word)
        let b = "The quick red fox jumps over the lazy cat. Rest.";
        assert!(hunks(a, b).iter().all(|h| h.parts.len() <= 2));
    }

    #[test]
    fn an_inserted_word_is_said_one_way() {
        // (diffed alone or with another change before it: "many " both times)
        let a = "Alpha lazy dog.\n\nA second paragraph, with words.";
        let b = "Alpha sleepy dog.\n\nA second paragraph, with many words.";
        let a2 = "Alpha sleepy dog.\n\nA second paragraph, with words.";
        let one = hunks(a, b).pop().unwrap().old_new();
        let two = hunks(a2, b).pop().unwrap().old_new();
        assert_eq!(one, two);
        assert_eq!(two.1, "many ");
    }

    #[test]
    fn a_word_changed_is_the_whole_word() {
        let a = "the permissiveness of it";
        let b = "the permissivenesz of it";
        let cs = changes(a, b);
        assert_eq!(cs.len(), 1);
        assert_eq!(&a[cs[0].a.0..cs[0].a.1], "permissiveness");
        assert_eq!(&b[cs[0].b.0..cs[0].b.1], "permissivenesz");
    }

    #[test]
    fn hunks_join_near_changes_and_apply_back() {
        let a = "One two three four five six seven eight nine ten.\n\nNext paragraph here.\n";
        let b = "One TWO three FOUR five six seven eight nine TEN.\n\nNext paragraph there.\n";
        let hs = hunks(a, b);
        // (two and four: one word between, joined; ten far: its own; the next paragraph its own)
        assert_eq!(hs.len(), 3, "{hs:?}");
        assert_eq!(
            hs[0].old_new(),
            ("two three four".to_owned(), "TWO three FOUR".to_owned())
        );
        assert_eq!(apply(a, b), b);
        // (a blank line between two changes keeps them apart)
        let a = "end a.\n\nb start";
        let b = "end X.\n\nY start";
        assert_eq!(hunks(a, b).len(), 2);
        assert_eq!(apply(a, b), b);
    }

    #[test]
    fn insertions_and_deletions_apply_back() {
        for (a, b) in [
            ("", "new file\n"),
            ("old file\n", ""),
            ("a b c", "a b c d"),
            ("a b c", "x a b c"),
            (
                "para one.\n\npara two.\n",
                "para one.\n\nA new paragraph, inserted.\n\npara two.\n",
            ),
            ("héllo wörld", "héllo wörld again"),
            ("x\\emph{a} y", "x\\textbf{a} y"),
        ] {
            assert_eq!(apply(a, b), b, "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn ranges_follow_edits() {
        // (a word edit before it: shifted; inside it: kept, grown)
        let a = "Alpha beta gamma delta epsilon.";
        let note = (a.find("gamma").unwrap(), a.find(" epsilon").unwrap());
        let b = "Alpha BETA-MAX gamma delta epsilon.";
        let r = map_range(&changes(a, b), note).unwrap();
        assert_eq!(&b[r.0..r.1], "gamma delta");
        let b = "Alpha beta gamma DELTA epsilon.";
        let r = map_range(&changes(a, b), note).unwrap();
        assert_eq!(&b[r.0..r.1], "gamma DELTA");
        // (a paragraph inserted before it)
        let b = "New first paragraph.\n\nAlpha beta gamma delta epsilon.";
        let r = map_range(&changes(a, b), note).unwrap();
        assert_eq!(&b[r.0..r.1], "gamma delta");
        // (its words all replaced: gone)
        let b = "Alpha beta omega epsilon.";
        assert_eq!(map_range(&changes(a, b), note), None);
        // (text inserted right at its start: left out)
        let b = "Alpha beta NEW gamma delta epsilon.";
        let r = map_range(&changes(a, b), note).unwrap();
        assert_eq!(&b[r.0..r.1], "gamma delta");
    }

    #[test]
    fn words_seen_on_a_page_are_found_in_the_source() {
        let t = "x \\emph{Chapter} one\ntext, with a \\textbf{ﬁne} line.";
        let r = find_seen(t, "Chapter one text", 0).unwrap();
        assert_eq!(&t[r.0..r.1], "Chapter} one\ntext");
        assert!(find_seen(t, "fine line", 0).is_some());
        assert!(shows("The quick brown", "quick  brown"));
        assert!(!shows("\\input{ch1}", "Chapter one"));
    }

    #[test]
    fn a_moved_quote_is_found_by_its_context() {
        let text = "x the cat sat. y the cat ran. z";
        let r = find_quote(text, "the cat", "y ", " ran", 0).unwrap();
        assert_eq!(r.0, text.find("y the").unwrap() + 2);
        assert_eq!(find_quote(text, "dog", "", "", 0), None);
        // (a short quote with no context: not trusted)
        assert_eq!(find_quote(text, "x", "", "", 0), None);
    }

    #[test]
    fn edited_ranges_name_their_lines() {
        let then = "a\nb\nc\nd\n";
        let now = "a\nB\nc\nD and more\n";
        // (two lines with one between: one range)
        let e = edited(then, now);
        assert_eq!(e.iter().map(|x| (x.2, x.3)).collect::<Vec<_>>(), [(2, 4)]);
        let then = "a\nb\nc\n\nx y z w\nd\n";
        let now = "a\nB\nc\n\nx y z w\nD\n";
        let e = edited(then, now);
        assert_eq!(
            e.iter().map(|x| (x.2, x.3)).collect::<Vec<_>>(),
            [(2, 2), (6, 6)]
        );
    }
}
