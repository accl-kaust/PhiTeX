//! A format's text in chunks (docs/spec-ini-chunks.md): cut at line ends
//! chosen by the lines' own bytes, so an edit moves only the boundaries
//! it touches; each chunk with a name that stays while its place does, so
//! a continuation that names a line of it (the chunk and the line) holds
//! through an edit anywhere else. A run reads the lines it reaches, each
//! one an element of its chunk's field: an edit asks again only the runs
//! that read the line it changed.

use phitex_defuse::{Field, Step, ValueId, Values, hash_bytes};

use crate::input::{Line, Seg};
use crate::run::R;

/// A line ends a chunk where its hash is 0 modulo this, from the chunk's
/// [`MIN_LINES`]th line on.
const EVERY: u64 = 16;
/// The fewest lines a chunk has before a line may end it.
const MIN_LINES: usize = 8;
/// The most lines a chunk has.
const MAX_LINES: usize = 256;

/// The first chunk's name: a text's node names its first line by it,
/// whatever its edits do (an edit keeps the names of the chunks it
/// touches, in order, [`rechunk`]).
pub(crate) const FIRST: u64 = 1;

/// One chunk of a text: its bytes `[start, end)` (whole lines, the last
/// one's newline in it), its first line's number (from 1) and its lines,
/// the hash of its text and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chunk {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub lines: u32,
    pub h: u64,
    pub id: u64,
}

/// A text's chunks, and the next name free.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chunks {
    pub cs: Vec<Chunk>,
    pub next: u64,
}

/// Whether line `l` (its bytes, without its newline) may end a chunk.
fn ends_chunk(l: &[u8]) -> bool {
    hash_bytes(0x6a63_6b65, l).is_multiple_of(EVERY)
}

/// The chunk of `b` that begins at `start`: its end and its lines.
fn next_chunk(b: &[u8], start: usize) -> (usize, usize) {
    let (mut at, mut n) = (start, 0usize);
    while at < b.len() {
        let e = b[at..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |k| at + k + 1);
        let l = &b[at..e];
        let l = l.strip_suffix(b"\n").unwrap_or(l);
        n += 1;
        at = e;
        if (n >= MIN_LINES && ends_chunk(l)) || n >= MAX_LINES {
            break;
        }
    }
    (at, n)
}

/// The chunk of `b` from `start` (its line `line`), its text hashed,
/// not named yet.
fn cut_at(b: &[u8], start: usize, line: u32) -> Chunk {
    let (end, n) = next_chunk(b, start);
    Chunk {
        start,
        end,
        line,
        lines: u32::try_from(n).unwrap_or(u32::MAX),
        h: hash_bytes(0x6a74_6578, &b[start..end]),
        id: 0,
    }
}

/// `text` cut into chunks ([`Chunk`]), in order, covering it all, named
/// from [`FIRST`] on.
pub(crate) fn chunks_of(text: &str) -> Chunks {
    let b = text.as_bytes();
    let mut cs = Vec::new();
    let (mut at, mut line, mut id) = (0usize, 1u32, FIRST);
    while at < b.len() {
        let mut c = cut_at(b, at, line);
        c.id = id;
        id += 1;
        at = c.end;
        line = line.saturating_add(c.lines);
        cs.push(c);
    }
    Chunks { cs, next: id }
}

/// The chunks of `text`, given `old`, those of `old_text`: cut again from
/// the chunk the first changed byte is in to the first boundary after
/// the change that is one of the old ones; the chunks after it the old
/// ones, moved. The chunks cut again take the names of the old ones they
/// replace, in order, any more new names: the bounds are [`chunks_of`]'s,
/// the names are the edits'.
pub(crate) fn rechunk(old: &Chunks, old_text: &str, text: &str) -> Chunks {
    let (a, b) = (old_text.as_bytes(), text.as_bytes());
    let old_cs = &old.cs;
    if old_cs.is_empty() || a.is_empty() || b.is_empty() {
        // (the first chunk [`FIRST`], the others new names)
        let mut c = chunks_of(text);
        let mut next = old.next.max(FIRST + 1);
        for x in c.cs.iter_mut().skip(1) {
            x.id = next;
            next += 1;
        }
        c.next = next;
        return c;
    }
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    if p == a.len() && p == b.len() {
        return old.clone();
    }
    let room = a.len().min(b.len()) - p;
    let s = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(room)
        .take_while(|(x, y)| x == y)
        .count();
    // (the old chunk holding the first changed byte; the last one where
    // the change is at the end, its end the text's)
    let i = old_cs.partition_point(|c| c.end <= p).min(old_cs.len() - 1);
    let mut cs: Vec<Chunk> = old_cs[..i].to_vec();
    // (the bytes and lines the change adds; the old chunks from the first
    // boundary in the unchanged end on, moved by them)
    let (old_tail, new_tail) = (a.len() - s, b.len() - s);
    let nl = |x: &[u8]| x.iter().fold(0usize, |n, &c| n + usize::from(c == b'\n'));
    let dl = i64::try_from(nl(&b[p..new_tail])).unwrap_or(0)
        - i64::try_from(nl(&a[p..old_tail])).unwrap_or(0);
    let (mut at, mut line) = (old_cs[i].start, old_cs[i].line);
    // (the old chunks the new ones replace: from `i` to `j`)
    let mut j = old_cs.len() - 1;
    while at < b.len() {
        let c = cut_at(b, at, line);
        at = c.end;
        line = line.saturating_add(c.lines);
        cs.push(c);
        if at >= new_tail && at < b.len() {
            let o = at - new_tail + old_tail;
            let k = old_cs.partition_point(|c| c.end < o);
            if old_cs.get(k).is_some_and(|c| c.end == o) {
                j = k;
                break;
            }
        }
    }
    let mut next = old.next;
    for (q, c) in cs[i..].iter_mut().enumerate() {
        c.id = if let Some(o) = old_cs.get(i + q).filter(|_| i + q <= j) {
            o.id
        } else {
            next += 1;
            next - 1
        };
    }
    cs.extend(old_cs[j + 1..].iter().map(|c| Chunk {
        start: c.start - old_tail + new_tail,
        end: c.end - old_tail + new_tail,
        line: u32::try_from(i64::from(c.line) + dl).unwrap_or(0),
        ..c.clone()
    }));
    Chunks { cs, next }
}

// ------------------------------------------------------- the fields

/// The environment's field of a chunk's lines (`jtext:<id>`): a list of
/// its lines, each a string with its newline.
pub(crate) fn text_field(v: &Values, id: u64) -> Field {
    v.field(&format!("jtext:{id:016x}"))
}

/// The field of the chunk after a chunk (`jnext:<id>`): its name, 0 at
/// the text's end.
pub(crate) fn next_field(v: &Values, id: u64) -> Field {
    v.field(&format!("jnext:{id:016x}"))
}

/// The field of a chunk's first line's number (`jline:<id>`).
pub(crate) fn line_field(v: &Values, id: u64) -> Field {
    v.field(&format!("jline:{id:016x}"))
}

/// The field of a chunk's byte offset in the text (`jpos:<id>`).
pub(crate) fn pos_field(v: &Values, id: u64) -> Field {
    v.field(&format!("jpos:{id:016x}"))
}

/// A diagnostic's file where its line is a format's text's, read from
/// its chunks (`crate::run::DiagPlace`): its place is the chunk's name
/// and the line in it, placed by the build ([`line_start`]).
pub(crate) const PLACE: &str = "\u{10FFFE}chunk";

/// The byte where line `k` of chunk `id` begins in `text`, the chunks
/// `cs`'s; `None` for a chunk or line they do not have.
pub(crate) fn line_start(cs: &Chunks, text: &str, id: u64, k: u32) -> Option<usize> {
    let c = cs.cs.iter().find(|c| c.id == id)?;
    let t = text.get(c.start..c.end)?;
    if k == 0 {
        return Some(c.start);
    }
    t.bytes()
        .enumerate()
        .filter(|&(_, b)| b == b'\n')
        .nth(k as usize - 1)
        .map(|(i, _)| c.start + i + 1)
        .filter(|&a| a < c.end)
}

/// A chunk's name as a value.
pub(crate) fn id_value(v: &Values, id: u64) -> ValueId {
    v.int(i64::from_ne_bytes(id.to_ne_bytes()))
}

/// The frame of a format's text from line `line` of chunk `id` on
/// (`["j", id, line]`), decoded as [`Seg::Chunk`].
pub(crate) fn frame(v: &Values, id: u64, line: u32) -> ValueId {
    v.list(vec![v.str("j"), id_value(v, id), v.int(i64::from(line))])
}

/// Line `k` of chunk `id`, its text `l` (with its newline, where it has
/// one), as the frames of the text's node (`crate::run::slice_args`)
/// decode: begun and ended, its trailing spaces gone; with its place.
fn line_of(l: &str, id: u64, k: u32) -> Line {
    let ends = l.ends_with('\n');
    let l = l.strip_suffix('\n').unwrap_or(l);
    let l = if ends {
        l.trim_end_matches([' ', '\r'])
    } else {
        l
    };
    Line {
        buf: l.chars().map(u32::from).collect(),
        loc: 0,
        state: crate::input::NEW_LINE,
        endch: -2,
        starts: true,
        ends,
        origin: None,
        lineno: 0,
        owner: None,
        rescanned: false,
        chars: false,
        src: None,
        chunk: Some((id, k)),
    }
}

/// A chunk's lines as its `jtext:` value.
fn text_value(v: &Values, text: &str, c: &Chunk) -> ValueId {
    v.list(
        text[c.start..c.end]
            .split_inclusive('\n')
            .map(|l| v.str(l))
            .collect(),
    )
}

/// A chunk's four fields' values (`text`: the text it is of; `next`: the
/// next chunk's name, 0 for none).
fn values(v: &Values, text: &str, c: &Chunk, next: u64) -> [(Field, ValueId); 4] {
    [
        (text_field(v, c.id), text_value(v, text, c)),
        (next_field(v, c.id), id_value(v, next)),
        (line_field(v, c.id), v.int(i64::from(c.line))),
        (
            pos_field(v, c.id),
            v.int(i64::try_from(c.start).unwrap_or(i64::MAX)),
        ),
    ]
}

/// The environment's fields of chunks `cs` of `text`.
pub(crate) fn fields(v: &Values, cs: &Chunks, text: &str) -> Vec<(Field, ValueId)> {
    let cs = &cs.cs;
    cs.iter()
        .enumerate()
        .flat_map(|(k, c)| values(v, text, c, cs.get(k + 1).map_or(0, |n| n.id)))
        .collect()
}

/// The environment's changes from chunks `old` to chunks `new` of
/// `text`: a chunk new, its fields; a chunk kept, those of its fields
/// that changed (its lines, the next chunk, its line, its place). A chunk
/// gone keeps its fields: a run that resumes in it before the run before
/// it is asked again reads the text it was cut from, not an end of text
/// (the run before it read what changed and is asked again, and its
/// continuation names the chunks as they are now).
pub(crate) fn changes(
    v: &Values,
    old: &Chunks,
    new: &Chunks,
    text: &str,
) -> Vec<(Field, Option<ValueId>)> {
    let before: phitex_defuse::FxHashMap<u64, (u64, u64, u32, usize)> = old
        .cs
        .iter()
        .enumerate()
        .map(|(k, c)| {
            (
                c.id,
                (c.h, old.cs.get(k + 1).map_or(0, |n| n.id), c.line, c.start),
            )
        })
        .collect();
    let mut out = Vec::new();
    for (k, c) in new.cs.iter().enumerate() {
        let next = new.cs.get(k + 1).map_or(0, |n| n.id);
        let Some(&(h, n, l, p)) = before.get(&c.id) else {
            out.extend(values(v, text, c, next).map(|(f, x)| (f, Some(x))));
            continue;
        };
        if h != c.h {
            out.push((text_field(v, c.id), Some(text_value(v, text, c))));
        }
        if n != next {
            out.push((next_field(v, c.id), Some(id_value(v, next))));
        }
        if l != c.line {
            out.push((line_field(v, c.id), Some(v.int(i64::from(c.line)))));
        }
        if p != c.start {
            out.push((
                pos_field(v, c.id),
                Some(v.int(i64::try_from(c.start).unwrap_or(i64::MAX))),
            ));
        }
    }
    out
}

// ------------------------------------------------------- the reader

impl crate::run::Run<'_, '_> {
    /// Line `k` of chunk `id` (an element of its `jtext:`), and whether
    /// the chunk has a line after it: each an environment read of that
    /// alone, so an edit of another line asks this run nothing.
    fn chunk_line(&mut self, id: u64, k: u32) -> R<Option<(Line, bool)>> {
        let v = self.v;
        let f = text_field(v, id);
        let p = self.cx.path(&[Step::Index(k)]);
        let Some(x) = self.cx.env_path(f, p)? else {
            return Ok(None);
        };
        let line = v.with_str(x, |t| line_of(t, id, k)).unwrap_or_default();
        let p = self.cx.path(&[Step::Index(k + 1), Step::Present]);
        let more = self
            .cx
            .env_path(f, p)?
            .and_then(|x| v.as_bool(x))
            .unwrap_or(false);
        Ok(Some((line, more)))
    }

    /// The chunk after chunk `id` (its `jnext:`, an environment read);
    /// `None` at the text's end.
    fn chunk_next(&mut self, id: u64) -> R<Option<u64>> {
        let v = self.v;
        let x = self.cx.env(next_field(v, id))?;
        Ok(x.and_then(|x| v.as_int(x))
            .map(|k| u64::from_ne_bytes(k.to_ne_bytes()))
            .filter(|&n| n != 0))
    }

    /// The chunk segment at the text's index `i` read: its line, then
    /// where the text goes on after it (the chunk's next line, or the next
    /// chunk's first, read only as far as whether there is one); at the
    /// text's end, the line alone. A chunk's segment names a line that is
    /// there when it is made, so whether input is left is known without
    /// a read.
    #[cold]
    #[inline(never)]
    pub(crate) fn read_chunk(&mut self, i: usize) -> R<()> {
        let Some(&Seg::Chunk(mut id, mut k)) = self.input.text.get(i) else {
            return Ok(());
        };
        let mut segs = Vec::with_capacity(2);
        loop {
            match self.chunk_line(id, k)? {
                Some((line, more)) => {
                    segs.push(Seg::Line(line));
                    if more {
                        segs.push(Seg::Chunk(id, k + 1));
                    } else if let Some(n) = self.chunk_next(id)? {
                        segs.push(Seg::Chunk(n, 0));
                    }
                    break;
                }
                // (a line its chunk no longer has: a continuation made
                // before an edit, whose run is asked again with the one
                // made after it; read on in the chunk after it)
                None => match self.chunk_next(id)? {
                    Some(n) => (id, k) = (n, 0),
                    None => break,
                },
            }
        }
        // (the places in the text after it move: where pulled siblings'
        // texts begin, and each segment's file line)
        let n = segs.len();
        self.input.text.splice(i..=i, segs);
        for p in self.pull_at.iter_mut().flatten() {
            if *p > i {
                *p = *p + n - 1;
            }
        }
        if i < self.cont_lines.len() {
            let c = self.cont_lines[i];
            self.cont_lines.splice(i..=i, std::iter::repeat_n(c, n));
        }
        Ok(())
    }

    /// The text's next segment, where it is a chunk's, read: a line to
    /// reach (or the text ends).
    #[cold]
    #[inline(never)]
    pub(crate) fn read_chunks_due(&mut self) -> R<()> {
        while self.input.chunk_due() {
            self.read_chunk(self.input.text_at)?;
        }
        Ok(())
    }
}

/// The chunk and line of the first segment of `segs` read from chunks
/// (a chunk's line read but not reached, or one not read yet).
pub(crate) fn chunk_at(segs: &[Seg]) -> Option<(u64, u32)> {
    segs.iter().find_map(|s| match s {
        Seg::Line(l) => l.chunk,
        &Seg::Chunk(id, k) => Some((id, k)),
        _ => None,
    })
}

/// Whether segment `s` is of a format's text read from chunks.
pub(crate) fn from_chunk(s: &Seg) -> bool {
    match s {
        Seg::Line(l) => l.chunk.is_some(),
        Seg::Chunk(..) => true,
        _ => false,
    }
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

    /// The chunks' bounds and texts, without their names.
    fn bounds(c: &Chunks) -> Vec<(usize, usize, u32, u32, u64)> {
        c.cs.iter()
            .map(|c| (c.start, c.end, c.line, c.lines, c.h))
            .collect()
    }

    #[test]
    fn chunks_cover_the_text_in_whole_lines() {
        let t = text(2000);
        let cs = chunks_of(&t);
        assert!(cs.cs.len() > 10, "{} chunks", cs.cs.len());
        let mut at = 0;
        let mut line = 1;
        for c in &cs.cs {
            assert_eq!(c.start, at);
            assert_eq!(c.line, line);
            assert!(c.end == t.len() || t.as_bytes()[c.end - 1] == b'\n');
            assert!(c.lines as usize <= MAX_LINES);
            at = c.end;
            line += c.lines;
        }
        assert_eq!(at, t.len());
        assert_eq!(cs.cs.first().map(|c| c.id), Some(FIRST));
        assert_eq!(chunks_of("").cs.len(), 0);
        assert_eq!(chunks_of("no newline").cs.len(), 1);
    }

    #[test]
    fn an_edit_moves_only_the_bounds_it_touches() {
        let t = text(2000);
        let a = chunks_of(&t);
        let at = t.find("line 1000 of").expect("the line");
        let mut u = t.clone();
        u.replace_range(at..at + 4, "LINE");
        let b = rechunk(&a, &t, &u);
        assert_eq!(a.cs.len(), b.cs.len());
        let changed = a.cs.iter().zip(&b.cs).filter(|(x, y)| x != y).count();
        assert!(changed <= 2, "{changed} chunks changed by a word's edit");
        assert!(a.cs.iter().zip(&b.cs).all(|(x, y)| x.id == y.id));
        // (a line inserted: the chunks after it keep their names and
        // their texts, their line numbers move)
        let mut v = t.clone();
        v.insert_str(at, "\\relax\n");
        let c = rechunk(&a, &t, &v);
        let last = |cs: &Chunks| cs.cs.last().map(|c| (c.id, c.h, c.line)).expect("a chunk");
        assert_eq!(last(&c).0, last(&a).0);
        assert_eq!(last(&c).1, last(&a).1);
        assert_eq!(last(&c).2, last(&a).2 + 1);
    }

    #[test]
    fn rechunk_cuts_as_chunks_of_and_names_each_once() {
        let t = text(2000);
        let a = chunks_of(&t);
        let at = t.find("line 1000 of").expect("the line");
        let one = "\\relax\n".repeat(MAX_LINES);
        let edits: Vec<String> = vec![
            {
                let mut u = t.clone();
                u.replace_range(at..at + 4, "LINE");
                u
            },
            {
                let mut u = t.clone();
                u.insert_str(at, "\\relax\n\\relax\n");
                u
            },
            {
                let mut u = t.clone();
                u.replace_range(at..at + 3000, "");
                u
            },
            {
                let mut u = t.clone();
                u.insert_str(at, &one);
                u
            },
            format!("{t}\\bye\n"),
            t[..t.len() - 40].to_owned(),
            format!("% a first line\n{t}"),
            t.clone(),
            t[at..].to_owned(),
            format!("{one}{t}{one}"),
            String::new(),
        ];
        for u in &edits {
            let b = rechunk(&a, &t, u);
            assert_eq!(bounds(&b), bounds(&chunks_of(u)));
            let back = rechunk(&b, u, &t);
            assert_eq!(bounds(&back), bounds(&a));
            for cs in [&b, &back] {
                let mut ids: Vec<u64> = cs.cs.iter().map(|c| c.id).collect();
                assert!(ids.iter().all(|&x| x < cs.next), "{ids:?} {}", cs.next);
                ids.sort_unstable();
                ids.dedup();
                assert_eq!(ids.len(), cs.cs.len());
                assert!(cs.cs.first().is_none_or(|c| c.id == FIRST));
            }
        }
    }
}
