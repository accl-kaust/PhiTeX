#![cfg_attr(not(test), allow(dead_code))] // (read from step 3 of docs/spec-ini-chunks.md on)
//! A format's text in chunks (docs/spec-ini-chunks.md §2.1): cut at
//! line ends chosen by the lines' own bytes, so an edit moves only the
//! boundaries it touches; each chunk named by its text (and its place
//! among chunks of the same text), so an edit renames only the chunks it
//! changed.

use phitex_defuse::{hash_bytes, hash_words};

/// A line ends a chunk where its hash is 0 modulo this, from the chunk's
/// [`MIN_LINES`]th line on.
const EVERY: u64 = 16;
/// The fewest lines a chunk has before a line may end it.
const MIN_LINES: usize = 8;
/// The most lines a chunk has.
const MAX_LINES: usize = 256;

/// One chunk of a text: its bytes `[start, end)` (whole lines, the last
/// one's newline in it), its first line's number (from 1) and its lines,
/// and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chunk {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub lines: u32,
    pub id: u64,
}

/// Whether line `l` (its bytes, without its newline) may end a chunk.
fn ends_chunk(l: &[u8]) -> bool {
    hash_bytes(0x6a63_6b65, l).is_multiple_of(EVERY)
}

/// `text` cut into chunks ([`Chunk`]), in order, covering it all.
pub(crate) fn chunks_of(text: &str) -> Vec<Chunk> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut line, mut n) = (0usize, 1u32, 0usize);
    let mut at = 0;
    while at < b.len() {
        let e = b[at..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |k| at + k + 1);
        let l = &b[at..e];
        let l = l.strip_suffix(b"\n").unwrap_or(l);
        n += 1;
        at = e;
        if (n >= MIN_LINES && ends_chunk(l)) || n >= MAX_LINES || at >= b.len() {
            let lines = u32::try_from(n).unwrap_or(u32::MAX);
            out.push(Chunk {
                start,
                end: at,
                line,
                lines,
                id: 0,
            });
            start = at;
            line = line.saturating_add(lines);
            n = 0;
        }
    }
    // (named by the text, and the place among the chunks of the same
    // text before it)
    let mut seen: phitex_defuse::FxHashMap<u64, u64> = phitex_defuse::FxHashMap::default();
    for c in &mut out {
        let h = hash_bytes(0x6a74_6578, &b[c.start..c.end]);
        let k = seen.entry(h).or_default();
        c.id = hash_words(h, &[*k]).max(1);
        *k += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(n: usize) -> String {
        use std::fmt::Write as _;
        (0..n).fold(String::new(), |mut s, k| {
            let _ = writeln!(s, "\\def\\m{k}{{line {k} of the format}}%");
            s
        })
    }

    #[test]
    fn chunks_cover_the_text_in_whole_lines() {
        let t = text(2000);
        let cs = chunks_of(&t);
        assert!(cs.len() > 10, "{} chunks", cs.len());
        let mut at = 0;
        let mut line = 1;
        for c in &cs {
            assert_eq!(c.start, at);
            assert_eq!(c.line, line);
            assert!(c.end == t.len() || t.as_bytes()[c.end - 1] == b'\n');
            assert!(c.lines as usize <= MAX_LINES);
            at = c.end;
            line += c.lines;
        }
        assert_eq!(at, t.len());
        assert_eq!(chunks_of("").len(), 0);
        assert_eq!(chunks_of("no newline").len(), 1);
    }

    #[test]
    fn an_edit_renames_only_the_chunk_it_is_in() {
        let t = text(2000);
        let a = chunks_of(&t);
        let at = t.find("line 1000 of").expect("the line");
        let mut u = t.clone();
        u.replace_range(at..at + 4, "LINE");
        let b = chunks_of(&u);
        let ids = |cs: &[Chunk]| cs.iter().map(|c| c.id).collect::<Vec<_>>();
        let (ia, ib) = (ids(&a), ids(&b));
        let changed = ia.iter().filter(|x| !ib.contains(x)).count();
        assert!(changed <= 2, "{changed} chunks renamed by a word's edit");
        // (a line inserted: the chunks after it keep their names, their
        // line numbers move)
        let mut v = t.clone();
        v.insert_str(at, "\\relax\n");
        let c = chunks_of(&v);
        let renamed = ia.iter().filter(|x| !ids(&c).contains(x)).count();
        assert!(renamed <= 2, "{renamed} chunks renamed by a line inserted");
        let last = |cs: &[Chunk]| cs.last().map(|c| (c.id, c.line)).expect("a chunk");
        assert_eq!(last(&c).0, last(&a).0);
        assert_eq!(last(&c).1, last(&a).1 + 1);
    }

    #[test]
    fn equal_chunks_have_names_of_their_own() {
        let one = "\\relax\n".repeat(MAX_LINES);
        let t = one.repeat(3);
        let cs = chunks_of(&t);
        let mut ids: Vec<u64> = cs.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), cs.len());
    }
}
