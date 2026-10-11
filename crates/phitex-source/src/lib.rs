//! Source text as a run of *units*, each with an identity ([`UnitId`])
//! that lives as long as its text is not edited.
//!
//! A place in the text is a [`Pos`]: a unit and an offset inside it. It
//! is opaque: the engine stores, hashes and compares positions, and asks
//! the [`Source`] about them, but never does arithmetic on them. So an
//! edit moves nothing: a unit it did not touch keeps its id, and every
//! `Pos` into it stays right. Where a position *is* (its line number, its
//! byte offset, its order) is computed on demand, never stored.
//!
//! What a unit is belongs to the [`Split`] given to the source: one line
//! ([`Lines`]) for now. An edit returns a [`Splice`]: the ids it removed
//! and the ids it made, so what read the old text is a lookup by id.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A unit's identity, unique across every [`Source`] in the process (a
/// position from an `\input` file never names a unit of the main one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitId(u64);

fn fresh() -> UnitId {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    UnitId(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A place in the text: a unit and the byte offset in it. Opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pos {
    unit: UnitId,
    off: u32,
}

impl Pos {
    /// The unit it is in (what a reader records as read).
    #[must_use]
    pub fn unit(self) -> UnitId {
        self.unit
    }
}

/// What a splitter may ask the engine about (unused by [`Lines`]); a
/// splitter that reads meaning (catcodes, LaTeX environments) gets it.
pub trait SplitHint {}

/// Where units end.
pub trait Split: Send + Sync {
    /// The ends of the units in `text` (each `> 0`, increasing, the last
    /// `text.len()` if `text` is not empty), and whether the last unit is
    /// closed: an unclosed one goes on in whatever text follows.
    fn split(&self, text: &str, hint: Option<&dyn SplitHint>) -> (Vec<usize>, bool);

    /// How many units before an edit it may change (a blank line added
    /// can end a paragraph before it; never a line).
    fn lookbehind(&self) -> usize {
        0
    }
}

/// One unit per line, its `\n` included.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lines;

impl Split for Lines {
    fn split(&self, text: &str, _: Option<&dyn SplitHint>) -> (Vec<usize>, bool) {
        let mut ends: Vec<usize> = text.match_indices('\n').map(|(i, _)| i + 1).collect();
        let closed = ends.last() == Some(&text.len()) || text.is_empty();
        if !closed {
            ends.push(text.len());
        }
        (ends, closed)
    }
}

/// What an edit did: the units it removed, the units it made in their
/// place (in order). Units it did not name are as they were.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Splice {
    pub removed: Vec<UnitId>,
    pub inserted: Vec<UnitId>,
}

#[derive(Clone, Debug)]
struct Unit {
    id: UnitId,
    text: Arc<str>,
}

/// A text as units.
pub struct Source {
    split: Box<dyn Split>,
    units: Vec<Unit>,
    /// id → its place in `units` (the order index: a B-tree or a tree of
    /// lengths when a splice shifting a `Vec` costs too much).
    index: HashMap<UnitId, usize>,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("units", &self.units)
            .finish_non_exhaustive()
    }
}

impl Source {
    /// `text` cut into lines.
    #[must_use]
    pub fn new(text: &str) -> Source {
        Source::with_split(text, Box::new(Lines))
    }

    #[must_use]
    pub fn with_split(text: &str, split: Box<dyn Split>) -> Source {
        let (ends, _) = split.split(text, None);
        let mut s = Source {
            split,
            units: Vec::with_capacity(ends.len()),
            index: HashMap::with_capacity(ends.len()),
        };
        let mut from = 0;
        for e in ends {
            s.units.push(Unit {
                id: fresh(),
                text: text[from..e].into(),
            });
            from = e;
        }
        s.reindex(0);
        s
    }

    fn reindex(&mut self, from: usize) {
        for (i, u) in self.units.iter().enumerate().skip(from) {
            self.index.insert(u.id, i);
        }
    }

    fn at(&self, u: UnitId) -> usize {
        *self.index.get(&u).expect("a unit of this source")
    }

    // ---- reading ----

    /// The first unit (`None`: the text is empty).
    #[must_use]
    pub fn first(&self) -> Option<UnitId> {
        self.units.first().map(|u| u.id)
    }

    /// The unit after `u`.
    #[must_use]
    pub fn next(&self, u: UnitId) -> Option<UnitId> {
        self.units.get(self.at(u) + 1).map(|u| u.id)
    }

    /// Whether `u` is (still) a unit of this source.
    #[must_use]
    pub fn contains(&self, u: UnitId) -> bool {
        self.index.contains_key(&u)
    }

    /// A unit's text, shared (a reader on another thread keeps it).
    #[must_use]
    pub fn text(&self, u: UnitId) -> &Arc<str> {
        &self.units[self.at(u)].text
    }

    /// The position `off` bytes into `u`.
    ///
    /// # Panics
    ///
    /// If `off` is past the unit's end.
    #[must_use]
    pub fn pos(&self, u: UnitId, off: usize) -> Pos {
        assert!(off <= self.text(u).len(), "an offset within the unit");
        Pos {
            unit: u,
            off: u32::try_from(off).expect("a unit under 4 GB"),
        }
    }

    /// The start of the text (`None`: it is empty).
    #[must_use]
    pub fn start(&self) -> Option<Pos> {
        self.first().map(|u| Pos { unit: u, off: 0 })
    }

    /// The offset of `p` in its unit (for a reader walking that unit's
    /// text; never to be stored apart from the unit).
    #[must_use]
    pub fn offset(&self, p: Pos) -> usize {
        p.off as usize
    }

    /// The text from `a` to `b`: borrowed when in one unit.
    #[must_use]
    pub fn slice(&self, a: Pos, b: Pos) -> Cow<'_, str> {
        let (i, j) = (self.at(a.unit), self.at(b.unit));
        if i == j {
            return Cow::Borrowed(&self.units[i].text[a.off as usize..b.off as usize]);
        }
        let mut s = String::from(&self.units[i].text[a.off as usize..]);
        for u in &self.units[i + 1..j] {
            s.push_str(&u.text);
        }
        s.push_str(&self.units[j].text[..b.off as usize]);
        Cow::Owned(s)
    }

    /// The whole text.
    #[must_use]
    pub fn to_text(&self) -> String {
        self.units.iter().map(|u| &*u.text).collect()
    }

    // ---- where things are (computed, never stored) ----

    /// Whether `a` comes before `b`.
    #[must_use]
    pub fn before(&self, a: Pos, b: Pos) -> bool {
        (self.at(a.unit), a.off) < (self.at(b.unit), b.off)
    }

    /// The 1-based line `p` is on.
    #[must_use]
    pub fn line_no(&self, p: Pos) -> usize {
        let i = self.at(p.unit);
        let before: usize = self.units[..i].iter().map(|u| lines_in(&u.text)).sum();
        before + lines_in(&self.units[i].text[..p.off as usize]) + 1
    }

    /// The byte offset of `p` in the whole text.
    #[must_use]
    pub fn byte(&self, p: Pos) -> usize {
        let i = self.at(p.unit);
        self.units[..i].iter().map(|u| u.text.len()).sum::<usize>() + p.off as usize
    }

    /// The position at byte `b` of the whole text (an editor's offset);
    /// the end of the text is the end of its last unit.
    ///
    /// # Panics
    ///
    /// If `b` is past the end, or the text is empty and `b > 0`.
    #[must_use]
    pub fn at_byte(&self, b: usize) -> Option<Pos> {
        let mut from = 0;
        for (i, u) in self.units.iter().enumerate() {
            let end = from + u.text.len();
            if b < end || (b == end && i + 1 == self.units.len()) {
                return Some(Pos {
                    unit: u.id,
                    off: u32::try_from(b - from).expect("a unit under 4 GB"),
                });
            }
            from = end;
        }
        assert!(b == 0, "a byte within the text");
        None
    }

    // ---- editing ----

    /// Replace the text `a..b` with `new`, and cut again from the unit `a`
    /// is in (and the splitter's lookbehind) until a unit ends where an
    /// old one did. Units cut again whose text did not change keep their
    /// ids, so only what changed is named.
    ///
    /// # Panics
    ///
    /// If `b` is before `a`, or either is not a position of this source.
    pub fn edit(&mut self, a: Pos, b: Pos, new: &str) -> Splice {
        assert!(!self.before(b, a), "a range in order");
        let i = self.at(a.unit).saturating_sub(self.split.lookbehind());
        let j = self.at(b.unit);
        let mut buf = String::new();
        for u in &self.units[i..self.at(a.unit)] {
            buf.push_str(&u.text);
        }
        buf.push_str(&self.units[self.at(a.unit)].text[..a.off as usize]);
        buf.push_str(new);
        buf.push_str(&self.units[j].text[b.off as usize..]);
        let mut k = j + 1;
        let ends = loop {
            let (ends, closed) = self.split.split(&buf, None);
            if closed || k == self.units.len() {
                break ends;
            }
            buf.push_str(&self.units[k].text);
            k += 1;
        };
        let mut pieces: Vec<&str> = Vec::with_capacity(ends.len());
        let mut from = 0;
        for e in ends {
            pieces.push(&buf[from..e]);
            from = e;
        }
        // (units at either end whose text came back keep their ids)
        let old = &self.units[i..k];
        let head = old
            .iter()
            .zip(&pieces)
            .take_while(|(u, p)| *u.text == ***p)
            .count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(pieces[head..].iter().rev())
            .take_while(|(u, p)| *u.text == ***p)
            .count();
        let (i, k) = (i + head, k - tail);
        let made: Vec<Unit> = pieces[head..pieces.len() - tail]
            .iter()
            .map(|p| Unit {
                id: fresh(),
                text: (*p).into(),
            })
            .collect();
        let splice = Splice {
            removed: self.units[i..k].iter().map(|u| u.id).collect(),
            inserted: made.iter().map(|u| u.id).collect(),
        };
        for u in &splice.removed {
            self.index.remove(u);
        }
        let shifted = made.len() != k - i;
        self.units.splice(i..k, made);
        if shifted {
            self.reindex(i);
        } else {
            let n = splice.inserted.len();
            for (o, u) in self.units[i..i + n].iter().enumerate() {
                self.index.insert(u.id, i + o);
            }
        }
        splice
    }

    /// Replace the whole text (a file the build wrote again): units whose
    /// text came back at either end keep their ids.
    pub fn replace(&mut self, text: &str) -> Splice {
        match (self.start(), self.units.last()) {
            (Some(a), Some(l)) => {
                let b = Pos {
                    unit: l.id,
                    off: u32::try_from(l.text.len()).expect("a unit under 4 GB"),
                };
                self.edit(a, b, text)
            }
            _ => {
                *self = Source::with_split(text, std::mem::replace(&mut self.split, Box::new(Lines)));
                Splice {
                    removed: Vec::new(),
                    inserted: self.units.iter().map(|u| u.id).collect(),
                }
            }
        }
    }

    /// The units in order (for a whole-text walk).
    pub fn units(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.units.iter().map(|u| u.id)
    }

    /// The byte range of `u` in the whole text.
    #[must_use]
    pub fn range(&self, u: UnitId) -> Range<usize> {
        let b = self.byte(Pos { unit: u, off: 0 });
        b..b + self.text(u).len()
    }
}

fn lines_in(s: &str) -> usize {
    s.bytes().filter(|&c| c == b'\n').count()
}

/// Reads a source's bytes forward, across units; within a unit a read is
/// an index.
#[derive(Clone, Debug)]
pub struct Cursor {
    unit: UnitId,
    text: Arc<str>,
    off: usize,
}

impl Cursor {
    /// A cursor at `p`.
    #[must_use]
    pub fn new(src: &Source, p: Pos) -> Cursor {
        Cursor {
            unit: p.unit,
            text: Arc::clone(src.text(p.unit)),
            off: p.off as usize,
        }
    }

    /// Where it is.
    #[must_use]
    pub fn pos(&self) -> Pos {
        Pos {
            unit: self.unit,
            off: u32::try_from(self.off).expect("a unit under 4 GB"),
        }
    }

    /// The unit it reads (what a reader records as read).
    #[must_use]
    pub fn unit(&self) -> UnitId {
        self.unit
    }

    /// The rest of the unit at hand.
    #[must_use]
    pub fn rest(&self) -> &str {
        &self.text[self.off..]
    }

    /// Move `n` bytes on, within the unit.
    ///
    /// # Panics
    ///
    /// Past the unit's end.
    pub fn bump(&mut self, n: usize) {
        assert!(self.off + n <= self.text.len(), "within the unit");
        self.off += n;
    }

    /// At the unit's end, move to the next unit; false at the text's end.
    pub fn next_unit(&mut self, src: &Source) -> bool {
        match src.next(self.unit) {
            Some(u) => {
                *self = Cursor {
                    unit: u,
                    text: Arc::clone(src.text(u)),
                    off: 0,
                };
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests;
