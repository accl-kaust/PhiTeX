//! A record's reads, coded: one sorted, delta-coded set per node
//! (docs/spec-guard-reads.md §2.1).
//!
//! A run's reads are a vector of [`ReadRec`] while it runs; its commit
//! codes them here, sorted by field (then kind, anchor or bounds, path,
//! answer), each entry a head byte and varints: about 9 bytes a read,
//! against 24 for a [`ReadRec`] kept as is and the reader records filed
//! besides. Every [`BLOCK`]th entry codes its field whole, and the
//! blocks' offsets lead the bytes, so the entries of one field are found
//! by a binary search ([`Reads::of_field`]). The run's order is not kept:
//! nothing reads a record's reads by their place in the run.

use crate::api::{NodeId, PathId, UnitId};
use crate::node::{Anchor, NO_VALUE, ReadKind, ReadRec};
use crate::values::{Field, ValueId};

/// Entries a block holds: its first codes its field whole.
const BLOCK: usize = 8;

/// The kinds' codes: the low four bits of an entry's head.
const K_ME: u8 = 0;
const K_BEFORE: u8 = 1;
const K_AFTER_OWN: u8 = 2;
const K_END: u8 = 3;
const K_FINAL: u8 = 4;
const K_ENV: u8 = 5;
const K_FOLD: u8 = 6;
const K_FOLD_INCL: u8 = 7;
const K_NEXT: u8 = 8;
const K_PLACED: u8 = 9;
const K_PLACED_GUESS: u8 = 10;
const K_FOLLOWING: u8 = 11;
const K_HOLE: u8 = 12;

/// The head's filed bit.
const H_FILED: u8 = 1 << 4;
/// The head's top three bits: `seen`'s tag, 7 for none.
const H_SEEN_SHIFT: u32 = 5;
const SEEN_NONE: u8 = 7;

const TAG_SHIFT: u32 = 61;
const PAYLOAD: u64 = (1 << TAG_SHIFT) - 1;
const T_INT: u64 = 2;
const T_SEQ: u64 = 3;

/// A read's kind as its code and two operands (the sort's and the
/// coding's).
fn code_of(k: &ReadKind) -> (u8, u64, u64) {
    let opt = |n: Option<NodeId>| n.map_or(0, |n| n.raw() + 1);
    match *k {
        ReadKind::Field { at, .. } => match at {
            Anchor::Me => (K_ME, 0, 0),
            Anchor::Before(n) => (K_BEFORE, n.raw(), 0),
            Anchor::AfterOwn(n) => (K_AFTER_OWN, n.raw(), 0),
            Anchor::End(u) => (K_END, u64::from(u.0), 0),
            Anchor::Final => (K_FINAL, 0, 0),
        },
        ReadKind::Env { .. } => (K_ENV, 0, 0),
        ReadKind::Fold { from, to, incl, .. } => (if incl { K_FOLD_INCL } else { K_FOLD }, opt(from), opt(to)),
        ReadKind::Next { after, .. } => (K_NEXT, opt(after), 0),
        ReadKind::Placed { i, guess } => (if guess { K_PLACED_GUESS } else { K_PLACED }, i as u64, 0),
        ReadKind::Following { i } => (K_FOLLOWING, i as u64, 0),
        ReadKind::Hole => (K_HOLE, 0, 0),
    }
}

/// Whether a kind's entries code a field.
fn has_field(code: u8) -> bool {
    code <= K_NEXT
}

/// How many operands a kind's entries code.
fn operands(code: u8) -> usize {
    match code {
        K_BEFORE | K_AFTER_OWN | K_END | K_NEXT | K_PLACED | K_PLACED_GUESS | K_FOLLOWING => 1,
        K_FOLD | K_FOLD_INCL => 2,
        _ => 0,
    }
}

fn kind_of(code: u8, f: Field, a: u64, b: u64) -> ReadKind {
    let opt = |x: u64| x.checked_sub(1).map(NodeId::from_raw);
    let ix = |x: u64| usize::try_from(x).unwrap_or(usize::MAX);
    match code {
        K_ME => ReadKind::Field { f, at: Anchor::Me },
        K_BEFORE => ReadKind::Field {
            f,
            at: Anchor::Before(NodeId::from_raw(a)),
        },
        K_AFTER_OWN => ReadKind::Field {
            f,
            at: Anchor::AfterOwn(NodeId::from_raw(a)),
        },
        K_END => ReadKind::Field {
            f,
            at: Anchor::End(UnitId(u32::try_from(a).unwrap_or(u32::MAX))),
        },
        K_FINAL => ReadKind::Field { f, at: Anchor::Final },
        K_ENV => ReadKind::Env { f },
        K_FOLD | K_FOLD_INCL => ReadKind::Fold {
            f,
            from: opt(a),
            to: opt(b),
            incl: code == K_FOLD_INCL,
        },
        K_NEXT => ReadKind::Next { f, after: opt(a) },
        K_PLACED | K_PLACED_GUESS => ReadKind::Placed {
            i: ix(a),
            guess: code == K_PLACED_GUESS,
        },
        K_FOLLOWING => ReadKind::Following { i: ix(a) },
        _ => ReadKind::Hole,
    }
}

fn put(out: &mut Vec<u8>, mut x: u64) {
    while x >= 0x80 {
        out.push((x as u8) | 0x80);
        x >>= 7;
    }
    out.push(x as u8);
}

fn get(b: &[u8], i: &mut usize) -> u64 {
    let mut x = 0u64;
    let mut s = 0;
    loop {
        let c = b[*i];
        *i += 1;
        x |= u64::from(c & 0x7f) << s;
        if c < 0x80 {
            return x;
        }
        s += 7;
    }
}

fn zig(x: i64) -> u64 {
    ((x << 1) ^ (x >> 63)) as u64
}

fn unzig(x: u64) -> i64 {
    ((x >> 1) as i64) ^ -((x & 1) as i64)
}

/// A 61-bit two's complement payload as an `i64`.
fn signed61(p: u64) -> i64 {
    ((p << 3) as i64) >> 3
}

/// A record's reads, coded (the module's doc).
#[derive(Default, Clone)]
pub(crate) struct Reads {
    n: u32,
    /// The log2 of its filter's bits (0: none).
    flog: u8,
    /// The blocks' offsets into the entries (4 bytes each, little
    /// endian), then the entries, then a filter of the fields read
    /// ([`Reads::may_read`]: about a byte a field, two bits set each).
    buf: Box<[u8]>,
}

/// A field's two bits in a filter of `1 << flog` bits.
fn filter_bits(f: u64, flog: u8) -> (usize, usize) {
    let h = (f ^ (f >> 29)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let sh = 64 - u32::from(flog);
    ((h >> sh) as usize, (h.rotate_left(32) >> sh) as usize)
}

impl std::fmt::Debug for Reads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reads({} in {} bytes)", self.n, self.buf.len())
    }
}

/// The sort's key of a read: field (none last), kind, operands, path,
/// what it saw, its epoch and filing (a total order: one record codes
/// one way).
type SortKey = (u64, u8, u64, u64, u32, u64, u64, bool);

fn sort_key(r: &ReadRec) -> SortKey {
    let (c, a, b) = code_of(r.kind());
    let f = r.kind().field().map_or(u64::MAX, |f| f.0.raw());
    (f, c, a, b, r.path().0, r.seen_raw(), r.epoch, r.filed())
}

impl Reads {
    /// The reads of a run, in the run's order, coded.
    pub(crate) fn new(v: &[ReadRec]) -> Reads {
        let n = v.len();
        if n == 0 {
            return Reads::default();
        }
        // (by field first, a pair of words each; then each run of one
        // field by the whole key: most fields are read once or twice)
        let mut by_field: Vec<(u64, u32)> = v
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    r.kind().field().map_or(u64::MAX, |f| f.0.raw()),
                    u32::try_from(i).expect("reads in a run"),
                )
            })
            .collect();
        by_field.sort_unstable();
        let mut keys: Vec<(SortKey, u32)> = by_field.iter().map(|&(_, i)| (sort_key(&v[i as usize]), i)).collect();
        let mut a = 0;
        while a < keys.len() {
            let f = keys[a].0.0;
            let b = a + keys[a..].iter().take_while(|x| x.0.0 == f).count();
            if b - a > 1 {
                keys[a..b].sort_unstable_by_key(|x| x.0);
            }
            a = b;
        }
        let blocks = n.div_ceil(BLOCK);
        let mut out: Vec<u8> = Vec::with_capacity(blocks * 4 + n * 10);
        out.resize(blocks * 4, 0);
        let base = blocks * 4;
        let mut prev = 0u64;
        for (j, &((f, code, a, b, path, seen, epoch, filed), _)) in keys.iter().enumerate() {
            if j % BLOCK == 0 {
                let off = u32::try_from(out.len() - base).expect("a record's reads fit 4 GB");
                out[(j / BLOCK) * 4..(j / BLOCK) * 4 + 4].copy_from_slice(&off.to_le_bytes());
                prev = 0;
            }
            let tag = if seen == NO_VALUE {
                SEEN_NONE
            } else {
                (seen >> TAG_SHIFT) as u8
            };
            out.push(code | if filed { H_FILED } else { 0 } | (tag << H_SEEN_SHIFT));
            if has_field(code) {
                put(&mut out, f - prev);
                prev = f;
            }
            match operands(code) {
                1 => put(&mut out, a),
                2 => {
                    put(&mut out, a);
                    put(&mut out, b);
                }
                _ => {}
            }
            put(&mut out, u64::from(path));
            if tag != SEEN_NONE {
                let p = seen & PAYLOAD;
                match u64::from(tag) {
                    T_INT => put(&mut out, zig(signed61(p))),
                    T_SEQ => out.extend_from_slice(&p.to_le_bytes()),
                    0 | 1 => put(&mut out, p),
                    // (a plain value: by its distance from the field's id,
                    // both counted up in one store)
                    _ => {
                        let fp = if has_field(code) { f & PAYLOAD } else { 0 };
                        put(&mut out, zig((p as i64).wrapping_sub(fp as i64)));
                    }
                }
            }
            put(&mut out, epoch.wrapping_add(1));
        }
        // (the filter of the fields read: a walk passes most nodes that
        // read none of its field, each a look at two bits)
        let mut fields = 0usize;
        let mut last = u64::MAX;
        for k in &keys {
            if k.0.0 != u64::MAX && k.0.0 != last {
                fields += 1;
                last = k.0.0;
            }
        }
        let flog = (fields * 8).next_power_of_two().max(64).trailing_zeros() as u8;
        let at = out.len();
        out.resize(at + (1usize << flog) / 8, 0);
        for k in &keys {
            if k.0.0 != u64::MAX {
                let (a, b) = filter_bits(k.0.0, flog);
                out[at + a / 8] |= 1 << (a % 8);
                out[at + b / 8] |= 1 << (b % 8);
            }
        }
        Reads {
            n: u32::try_from(n).expect("reads in a run"),
            flog,
            buf: out.into_boxed_slice(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.n as usize
    }

    /// Whether it may read field `f` (none of the fields it reads gives
    /// false).
    pub(crate) fn may_read(&self, f: Field) -> bool {
        if self.n == 0 {
            return false;
        }
        if self.flog == 0 {
            return true;
        }
        let at = self.buf.len() - (1usize << self.flog) / 8;
        let (a, b) = filter_bits(f.0.raw(), self.flog);
        self.buf[at + a / 8] & (1 << (a % 8)) != 0 && self.buf[at + b / 8] & (1 << (b % 8)) != 0
    }

    /// Bytes held (the measure).
    pub(crate) fn bytes(&self) -> usize {
        self.buf.len()
    }

    fn blocks(&self) -> usize {
        self.len().div_ceil(BLOCK)
    }

    fn block_at(&self, k: usize) -> usize {
        let b = &self.buf[k * 4..k * 4 + 4];
        self.blocks() * 4 + u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
    }

    /// Its reads in set order (by field).
    pub(crate) fn iter(&self) -> Entries<'_> {
        Entries {
            buf: &self.buf,
            at: if self.n == 0 { 0 } else { self.block_at(0) },
            j: 0,
            n: self.len(),
            prev: 0,
        }
    }

    /// Its reads of field `f` (`Env`, `Fold` and `Next` reads too), in
    /// set order: a binary search over the blocks, then their entries.
    pub(crate) fn of_field(&self, f: Field) -> impl Iterator<Item = ReadRec> + '_ {
        let key = f.0.raw();
        // (the first block whose first entry's field is past `f`; the
        // field's entries begin in the block before it)
        let ok = self.n != 0 && self.may_read(f);
        let (mut lo, mut hi) = (0, if ok { self.blocks() } else { 0 });
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            if self.first_field(mid) <= key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        // (`lo` blocks start at or before `f`: the last of them may hold
        // its first entry, or one before it a run of it that goes on)
        let mut k = lo.saturating_sub(1);
        while k > 0 && self.first_field(k) == key {
            k -= 1;
        }
        let it = if !ok {
            Entries {
                buf: &self.buf,
                at: 0,
                j: 0,
                n: 0,
                prev: 0,
            }
        } else {
            Entries {
                buf: &self.buf,
                at: self.block_at(k),
                j: k * BLOCK,
                n: self.len(),
                prev: 0,
            }
        };
        it.skip_while(move |r| r.kind().field().is_none_or(|g| g.0.raw() < key))
            .take_while(move |r| r.kind().field().is_some_and(|g| g.0.raw() == key))
    }

    /// The field of block `k`'s first entry (`u64::MAX` for one of none).
    fn first_field(&self, k: usize) -> u64 {
        let mut i = self.block_at(k);
        let code = self.buf[i] & 0x0f;
        i += 1;
        if has_field(code) {
            get(&self.buf, &mut i)
        } else {
            u64::MAX
        }
    }
}

/// [`Reads::iter`]'s iterator.
pub(crate) struct Entries<'a> {
    buf: &'a [u8],
    at: usize,
    j: usize,
    n: usize,
    prev: u64,
}

impl Iterator for Entries<'_> {
    type Item = ReadRec;

    fn next(&mut self) -> Option<ReadRec> {
        if self.j >= self.n {
            return None;
        }
        if self.j.is_multiple_of(BLOCK) {
            self.prev = 0;
        }
        self.j += 1;
        let b = self.buf;
        let mut i = self.at;
        let head = b[i];
        i += 1;
        let code = head & 0x0f;
        let filed = head & H_FILED != 0;
        let tag = head >> H_SEEN_SHIFT;
        let f = if has_field(code) {
            self.prev += get(b, &mut i);
            self.prev
        } else {
            0
        };
        let (a, bb) = match operands(code) {
            1 => (get(b, &mut i), 0),
            2 => {
                let a = get(b, &mut i);
                (a, get(b, &mut i))
            }
            _ => (0, 0),
        };
        let path = PathId(u32::try_from(get(b, &mut i)).unwrap_or(u32::MAX));
        let seen = if tag == SEEN_NONE {
            NO_VALUE
        } else {
            let t = u64::from(tag);
            let p = match t {
                T_INT => (unzig(get(b, &mut i)) as u64) & PAYLOAD,
                T_SEQ => {
                    let mut w = [0u8; 8];
                    w.copy_from_slice(&b[i..i + 8]);
                    i += 8;
                    u64::from_le_bytes(w)
                }
                0 | 1 => get(b, &mut i),
                _ => {
                    let fp = if has_field(code) { f & PAYLOAD } else { 0 };
                    ((fp as i64).wrapping_add(unzig(get(b, &mut i))) as u64) & PAYLOAD
                }
            };
            (t << TAG_SHIFT) | p
        };
        let epoch = get(b, &mut i).wrapping_sub(1);
        self.at = i;
        let kind = kind_of(code, Field(ValueId::from_raw(f)), a, bb);
        Some(ReadRec::from_parts(kind, path, filed, epoch, seen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: u64) -> Field {
        Field(ValueId::from_raw((4 << TAG_SHIFT) | (1 << 40) | x))
    }

    fn sample() -> Vec<ReadRec> {
        let mut v = Vec::new();
        let seens = [
            None,
            Some(ValueId::from_raw((2 << TAG_SHIFT) | (PAYLOAD & (-5i64 as u64)))),
            Some(ValueId::from_raw((3 << TAG_SHIFT) | 0x0123_4567_89ab_cdef)),
            Some(ValueId::from_raw((4 << TAG_SHIFT) | (1 << 40) | 77)),
            Some(ValueId::from_raw((5 << TAG_SHIFT) | (1 << 40) | 3)),
            Some(ValueId::UNIT),
            Some(ValueId::from_raw((1 << TAG_SHIFT) | 1)),
        ];
        for i in 0..300u64 {
            let fx = f((i * 7919) % 101);
            let kind = match i % 13 {
                0 => ReadKind::Field { f: fx, at: Anchor::Me },
                1 => ReadKind::Field {
                    f: fx,
                    at: Anchor::Before(NodeId::from_raw(i)),
                },
                2 => ReadKind::Field {
                    f: fx,
                    at: Anchor::AfterOwn(NodeId::from_raw(3)),
                },
                3 => ReadKind::Field {
                    f: fx,
                    at: Anchor::End(UnitId(2)),
                },
                4 => ReadKind::Field {
                    f: fx,
                    at: Anchor::Final,
                },
                5 => ReadKind::Env { f: fx },
                6 => ReadKind::Fold {
                    f: fx,
                    from: None,
                    to: Some(NodeId::from_raw(i)),
                    incl: i % 2 == 0,
                },
                7 => ReadKind::Next {
                    f: fx,
                    after: Some(NodeId::from_raw(0)),
                },
                8 => ReadKind::Placed {
                    i: i as usize,
                    guess: i % 3 == 0,
                },
                9 => ReadKind::Following { i: 4 },
                10 => ReadKind::Hole,
                _ => ReadKind::Field { f: fx, at: Anchor::Me },
            };
            let epoch = if kind.field().is_some() { i * 31 } else { u64::MAX };
            let mut r = ReadRec::new(kind, PathId((i % 5) as u32), seens[(i % 7) as usize], epoch);
            r.set_filed(i % 4 == 0);
            v.push(r);
        }
        v
    }

    #[test]
    fn a_record_s_reads_come_back_as_they_were_made_and_by_field() {
        let v = sample();
        let r = Reads::new(&v);
        let k = |r: &ReadRec| format!("{r:?}");
        assert_eq!(r.len(), v.len());
        let mut all: Vec<ReadRec> = r.iter().collect();
        let mut want = v.clone();
        all.sort_by_key(k);
        want.sort_by_key(k);
        assert_eq!(all, want);
        for x in 0..101 {
            let mut want: Vec<ReadRec> = v.iter().filter(|r| r.kind().field() == Some(f(x))).copied().collect();
            let mut got: Vec<ReadRec> = r.of_field(f(x)).collect();
            want.sort_by_key(k);
            got.sort_by_key(k);
            assert_eq!(got, want, "field {x}");
        }
        assert_eq!(r.of_field(f(500)).count(), 0);
        assert!(Reads::new(&[]).iter().next().is_none());
    }
}
