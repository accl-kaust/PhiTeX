//! Glyph origins (DESIGN 4.4): where each glyph came from in the sources,
//! a side channel beside the nodes.
//!
//! An origin ([`Org`]) is a byte range of a *data* (one version of a
//! source file's contents, numbered by the engine), or none. A node holds
//! no origin as part of its value: a glyph run and a ligature hold a
//! handle into an [`OrgTable`], outside their equality and hashing, so a
//! node's version, and every SSA version made from it, is the same with
//! origins on or off. A run of `n` characters with handle `h` has its
//! characters' origins at entries `h..h+n` of the table (handle 0: none);
//! a ligature's handle is one entry, the range of all its characters.
//!
//! The table only grows: an entry, once made, means the same place for
//! as long as anything holds its handle (an old step's record, a page's
//! list of glyphs). Where the place is now, after edits of its source, is
//! the engine's to say when it is asked (`partex-core`'s `srcmap`): an
//! entry is mapped through the edits of its data and written back,
//! never made again.

use alloc::vec::Vec;

use crate::node::{FontId, Glyphs, Node};

/// A place in the sources: a byte range of a data, possibly
/// *synthesized* (a glyph a macro body, `\the`, a counter or a break
/// hyphen made, given the range of the call that made it), or none.
///
/// Packed in 64 bits. Short (bit 62 clear): bit 63 synthesized, bits
/// 40–61 the data plus one, bits 32–39 the range's length, bits 0–31 its
/// start. Long (bit 62 set): bit 63 synthesized, bits 0–39 an index into
/// the table's long ranges, which hold the data, start and end.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub struct Org(u64);

const SYNTH: u64 = 1 << 63;
const LONG: u64 = 1 << 62;
const DATA_SHIFT: u32 = 40;
const DATA_MASK: u64 = (1 << 22) - 1;
const LEN_SHIFT: u32 = 32;

impl Org {
    /// No origin.
    pub const NONE: Org = Org(0);

    /// The most datas a short origin can name.
    pub const MAX_DATA: u32 = (1 << 22) - 2;

    #[must_use]
    pub fn is_none(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn synthesized(self) -> bool {
        self.0 & SYNTH != 0
    }

    /// The packed bits (for a list of origins kept as numbers).
    #[must_use]
    pub fn bits(self) -> u64 {
        self.0
    }

    /// An origin from [`Org::bits`].
    #[must_use]
    pub fn from_bits(b: u64) -> Org {
        Org(b)
    }
}

/// An origin's place: its data, its byte range and whether it was
/// synthesized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub data: u32,
    pub start: u32,
    pub end: u32,
    pub synthesized: bool,
}

/// A side-channel handle inside a node (an [`OrgTable`] index): never part
/// of the node's value, so any two are equal and none is hashed.
#[derive(Clone, Copy, Default, Debug)]
pub struct Side(pub u32);

impl PartialEq for Side {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Side {}

impl core::hash::Hash for Side {
    fn hash<H: core::hash::Hasher>(&self, _: &mut H) {}
}

/// A handle means nothing outside the run that made it: none is saved.
impl crate::persist::Persist for Side {
    fn save(&self, _: &mut crate::persist::Saver) {}
    fn load(_: &mut crate::persist::Loader) -> Option<Self> {
        Some(Side(0))
    }
}

/// The origins of the glyphs and tokens a build made, by handle.
#[derive(Clone, Debug)]
pub struct OrgTable {
    /// Entry 0 is none.
    entries: Vec<Org>,
    /// The long ranges: data, start, end.
    long: Vec<(u32, u32, u32)>,
}

impl Default for OrgTable {
    fn default() -> Self {
        Self::new()
    }
}

impl OrgTable {
    #[must_use]
    pub fn new() -> Self {
        OrgTable {
            entries: alloc::vec![Org::NONE],
            long: Vec::new(),
        }
    }

    /// How many entries there are (entry 0 included).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.len() <= 1
    }

    /// Entry `h` (none past the end).
    #[inline]
    #[must_use]
    pub fn get(&self, h: u32) -> Org {
        self.entries.get(h as usize).copied().unwrap_or(Org::NONE)
    }

    /// A new entry holding `o`: its handle.
    pub fn push(&mut self, o: Org) -> u32 {
        let h = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        self.entries.push(o);
        h
    }

    /// Entry `h` set to `o` (the same place, as an edit moved it).
    pub fn set(&mut self, h: u32, o: Org) {
        if let Some(e) = self.entries.get_mut(h as usize) {
            *e = o;
        }
    }

    /// The origin of bytes `start..end` of data `data`.
    pub fn range(&mut self, data: u32, start: u32, end: u32, synthesized: bool) -> Org {
        let end = end.max(start);
        let synth = if synthesized { SYNTH } else { 0 };
        let len = end - start;
        if data <= Org::MAX_DATA && len < 256 {
            return Org(synth
                | (u64::from(data + 1) << DATA_SHIFT)
                | (u64::from(len) << LEN_SHIFT)
                | u64::from(start));
        }
        let i = self.long.len() as u64;
        self.long.push((data, start, end));
        Org(synth | LONG | i)
    }

    /// Where `o` is, if it is somewhere.
    #[must_use]
    pub fn place(&self, o: Org) -> Option<Place> {
        if o.is_none() {
            return None;
        }
        let synthesized = o.synthesized();
        if o.0 & LONG != 0 {
            let i = usize::try_from(o.0 & ((1 << 40) - 1)).ok()?;
            let &(data, start, end) = self.long.get(i)?;
            return Some(Place {
                data,
                start,
                end,
                synthesized,
            });
        }
        let d = (o.0 >> DATA_SHIFT) & DATA_MASK;
        let data = u32::try_from(d.checked_sub(1)?).ok()?;
        let start = u32::try_from(o.0 & 0xffff_ffff).unwrap_or(0);
        let len = u32::try_from((o.0 >> LEN_SHIFT) & 0xff).unwrap_or(0);
        Some(Place {
            data,
            start,
            end: start + len,
            synthesized,
        })
    }

    /// `o` at place `p` (an edit moved it): a short origin made again, a
    /// long one's range changed where it is kept (every copy of it moves).
    #[must_use]
    pub fn moved(&mut self, o: Org, p: Place) -> Org {
        if o.0 & LONG != 0 {
            if let Some(x) = usize::try_from(o.0 & ((1 << 40) - 1))
                .ok()
                .and_then(|i| self.long.get_mut(i))
            {
                *x = (p.data, p.start, p.end.max(p.start));
            }
            return o;
        }
        self.range(p.data, p.start, p.end, p.synthesized)
    }

    /// `o`, synthesized.
    #[must_use]
    pub fn synth(o: Org) -> Org {
        if o.is_none() { o } else { Org(o.0 | SYNTH) }
    }

    /// The origin of a ligature of glyphs from `a` and `b`: the range
    /// that covers both where they are in one data, else `a`'s (or `b`'s,
    /// if `a` is none); synthesized if either is.
    pub fn union(&mut self, a: Org, b: Org) -> Org {
        let (Some(x), Some(y)) = (self.place(a), self.place(b)) else {
            return if a.is_none() { b } else { a };
        };
        let synthesized = x.synthesized || y.synthesized;
        if x.data != y.data {
            return if synthesized { Self::synth(a) } else { a };
        }
        self.range(x.data, x.start.min(y.start), x.end.max(y.end), synthesized)
    }

    /// The handle of a run that held `n` characters at handle `h`, after
    /// a character with origin `o` was added to it: the run's entries
    /// stay together, the run's own if they end the table, else a copy.
    pub fn run_push(&mut self, h: u32, n: usize, o: Org) -> u32 {
        if h == 0 {
            if o.is_none() {
                return 0;
            }
            let start = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
            self.entries.extend(core::iter::repeat_n(Org::NONE, n));
            self.entries.push(o);
            return start;
        }
        let h0 = h as usize;
        if h0 + n == self.entries.len() {
            self.entries.push(o);
            return h;
        }
        let start = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        for i in h0..h0 + n {
            let e = self.entries.get(i).copied().unwrap_or(Org::NONE);
            self.entries.push(e);
        }
        self.entries.push(o);
        start
    }

    /// The handle of a token list's origins `orgs` (0 if all are none).
    pub fn push_list(&mut self, orgs: &[Org]) -> u32 {
        if orgs.iter().all(|o| o.is_none()) {
            return 0;
        }
        let start = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        self.entries.extend_from_slice(orgs);
        start
    }

    /// Bytes used, roughly (a report).
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.entries.capacity() * 8 + self.long.capacity() * 12
    }
}

/// The origin of character `i` of run `g`.
#[inline]
#[must_use]
pub fn char_org(t: &OrgTable, g: &Glyphs, i: usize) -> Org {
    match g.org() {
        0 => Org::NONE,
        h => t.get(h + u32::try_from(i).unwrap_or(0)),
    }
}

/// Append character `ch` of `font`, from `o`, to `list`, keeping runs
/// canonical (`node::push_char`'s rule) and each run's origins together.
pub fn push_char_org(list: &mut Vec<Node>, font: FontId, ch: u8, o: Org, t: &mut OrgTable) {
    if let Some(Node::Glyphs(g)) = list.last_mut()
        && g.font == font
        && !g.is_full()
    {
        let h = t.run_push(g.org(), g.chars().len(), o);
        g.push(ch);
        g.set_org(h);
        return;
    }
    let h = if o.is_none() { 0 } else { t.push(o) };
    list.push(Node::Glyphs(Glyphs::one_at(font, ch, h)));
}

/// Append node `n` to `list`, merging a glyph run into the run before
/// it (canonical runs), its characters' origins kept: with `t`, through
/// the table; without, as `node::push_char` does.
pub fn append_node(list: &mut Vec<Node>, n: Node, t: Option<&mut OrgTable>) {
    match n {
        // a run that cannot merge with the last one stays canonical
        Node::Glyphs(g) if !matches!(list.last(), Some(Node::Glyphs(h)) if h.font == g.font && !h.is_full()) =>
        {
            list.push(Node::Glyphs(g));
        }
        Node::Glyphs(g) => match t {
            Some(t) => {
                for (i, &c) in g.chars().iter().enumerate() {
                    let o = char_org(t, &g, i);
                    push_char_org(list, g.font, c, o, t);
                }
            }
            None => {
                for &c in g.chars() {
                    crate::node::push_char(list, g.font, c);
                }
            }
        },
        n => list.push(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_round_trip() {
        let mut t = OrgTable::new();
        let a = t.range(3, 100, 104, false);
        assert_eq!(
            t.place(a),
            Some(Place {
                data: 3,
                start: 100,
                end: 104,
                synthesized: false
            })
        );
        let b = t.range(7, 10, 5000, true);
        assert_eq!(
            t.place(b),
            Some(Place {
                data: 7,
                start: 10,
                end: 5000,
                synthesized: true
            })
        );
        assert_eq!(t.place(Org::NONE), None);
        let c = t.range(3, 90, 91, false);
        let u = t.union(a, c);
        assert_eq!(t.place(u).map(|p| (p.start, p.end)), Some((90, 104)));
    }

    #[test]
    fn runs_keep_their_origins_together() {
        let mut t = OrgTable::new();
        let f = FontId(1);
        let mut list = Vec::new();
        let o = |t: &mut OrgTable, s: u32| t.range(0, s, s + 1, false);
        for s in 0..20 {
            let x = o(&mut t, s);
            push_char_org(&mut list, f, b'a', x, &mut t);
        }
        // (two runs: 15 and 5 characters)
        let starts: Vec<u32> = list
            .iter()
            .flat_map(|n| match n {
                Node::Glyphs(g) => (0..g.chars().len())
                    .map(|i| t.place(char_org(&t, g, i)).map_or(99, |p| p.start))
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(starts, (0..20).collect::<Vec<_>>());
        // a copy of the list extended: the copy's run is copied, the
        // first's entries stay
        let mut copy = list.clone();
        let x = o(&mut t, 50);
        push_char_org(&mut copy, f, b'b', x, &mut t);
        let y = o(&mut t, 60);
        push_char_org(&mut list, f, b'c', y, &mut t);
        let last = |l: &[Node], t: &OrgTable| match l.last() {
            Some(Node::Glyphs(g)) => t
                .place(char_org(t, g, g.chars().len() - 1))
                .map(|p| p.start),
            _ => None,
        };
        assert_eq!(last(&copy, &t), Some(50));
        assert_eq!(last(&list, &t), Some(60));
        // equal by characters, whatever the handles
        assert_eq!(list.len(), copy.len());
        assert!(node_eq_but_last(&list, &copy));
    }

    fn node_eq_but_last(a: &[Node], b: &[Node]) -> bool {
        a[..a.len() - 1] == b[..b.len() - 1]
    }
}
