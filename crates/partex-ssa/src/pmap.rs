//! A persistent map: a hash array mapped trie (32-way, path copying,
//! structural sharing).
//!
//! Each node carries a version of the entries beneath it, the wrapping
//! sum of one hash per entry (of the key and the value's version). The
//! sum is independent of insertion order and of the trie's shape, so a
//! write updates the versions on its path in O(1) each, and two maps
//! with equal content have equal versions.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::Hash;

use crate::hash::Version;
use crate::value::Value;

const BITS: u32 = 5;
const MASK: u64 = 31;

#[derive(Clone)]
enum Entry<K, V> {
    Leaf(u128, K, V),
    Sub(Arc<HNode<K, V>>),
    /// Keys whose 64-bit hashes are equal.
    Collide(Vec<(u128, K, V)>),
}

#[derive(Clone)]
struct HNode<K, V> {
    bitmap: u32,
    entries: Vec<Entry<K, V>>,
    sum: u128,
}

impl<K, V> HNode<K, V> {
    fn empty() -> Self {
        HNode {
            bitmap: 0,
            entries: Vec::new(),
            sum: 0,
        }
    }
}

#[inline]
fn contribution(kh: u128, v: Version) -> u128 {
    Version::node(0x0065_6e74_7279, &[Version(kh), v]).0
}

#[inline]
fn slot(h: u128, level: u32) -> u32 {
    // Truncation intended: five bits of the low half.
    #[allow(clippy::cast_possible_truncation)]
    let s = ((h as u64) >> (level * BITS) & MASK) as u32;
    s
}

#[inline]
fn low(h: u128) -> u64 {
    // Truncation intended.
    #[allow(clippy::cast_possible_truncation)]
    let l = h as u64;
    l
}

/// A persistent map from keys to values.
#[derive(Clone)]
pub struct PMap<K, V> {
    root: Option<Arc<HNode<K, V>>>,
    len: usize,
}

impl<K: Clone + Eq + Hash, V: Value> Default for PMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + Eq + Hash, V: Value> PMap<K, V> {
    #[must_use]
    pub fn new() -> Self {
        PMap { root: None, len: 0 }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The version of the whole map.
    #[must_use]
    pub fn version(&self) -> Version {
        let sum = self.root.as_ref().map_or(0, |r| r.sum);
        Version::node(0x6d61_7000 ^ self.len as u64, &[Version(sum)])
    }

    #[must_use]
    pub fn get(&self, k: &K) -> Option<&V> {
        let h = Version::of(k).0;
        let mut n = self.root.as_deref()?;
        let mut level = 0;
        loop {
            let bit = 1u32 << slot(h, level);
            if n.bitmap & bit == 0 {
                return None;
            }
            let i = (n.bitmap & (bit - 1)).count_ones() as usize;
            match &n.entries[i] {
                Entry::Leaf(eh, ek, v) => return (*eh == h && ek == k).then_some(v),
                Entry::Sub(s) => {
                    n = s;
                    level += 1;
                }
                Entry::Collide(xs) => {
                    return xs
                        .iter()
                        .find(|(eh, ek, _)| *eh == h && ek == k)
                        .map(|e| &e.2);
                }
            }
        }
    }

    /// Bind `k` to `v`; returns the previous value.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let h = Version::of(&k).0;
        let root = Arc::make_mut(self.root.get_or_insert_with(|| Arc::new(HNode::empty())));
        let old = insert(root, 0, h, k, v);
        if old.is_none() {
            self.len += 1;
        }
        old
    }

    /// Remove `k`; returns its value.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        // Look before copying the path.
        self.get(k)?;
        let h = Version::of(k).0;
        let root = Arc::make_mut(self.root.as_mut().expect("root"));
        let old = remove(root, 0, h, k);
        if old.is_some() {
            self.len -= 1;
            if self.len == 0 {
                self.root = None;
            }
        }
        old
    }

    /// The entries, in an order fixed by the keys' hashes.
    #[must_use]
    pub fn entries(&self) -> Vec<(&K, &V)> {
        fn walk<'a, K, V>(n: &'a HNode<K, V>, out: &mut Vec<(&'a K, &'a V)>) {
            for e in &n.entries {
                match e {
                    Entry::Leaf(_, k, v) => out.push((k, v)),
                    Entry::Sub(s) => walk(s, out),
                    Entry::Collide(xs) => out.extend(xs.iter().map(|(_, k, v)| (k, v))),
                }
            }
        }
        let mut out = Vec::with_capacity(self.len);
        if let Some(r) = &self.root {
            walk(r, &mut out);
        }
        out
    }
}

fn insert<K: Clone + Eq, V: Value>(
    n: &mut HNode<K, V>,
    level: u32,
    h: u128,
    k: K,
    v: V,
) -> Option<V> {
    let bit = 1u32 << slot(h, level);
    let i = (n.bitmap & (bit - 1)).count_ones() as usize;
    let new_c = contribution(h, v.version());
    if n.bitmap & bit == 0 {
        n.bitmap |= bit;
        n.entries.insert(i, Entry::Leaf(h, k, v));
        n.sum = n.sum.wrapping_add(new_c);
        return None;
    }
    let before = entry_sum(&n.entries[i]);
    if let Entry::Leaf(eh, ek, _) = &n.entries[i]
        && !(*eh == h && *ek == k)
    {
        let Entry::Leaf(eh, ek, ev) =
            core::mem::replace(&mut n.entries[i], Entry::Collide(Vec::new()))
        else {
            unreachable!()
        };
        if low(eh) == low(h) {
            n.entries[i] = Entry::Collide(alloc::vec![(eh, ek, ev), (h, k, v)]);
        } else {
            let mut sub = HNode::empty();
            insert(&mut sub, level + 1, eh, ek, ev);
            insert(&mut sub, level + 1, h, k, v);
            n.entries[i] = Entry::Sub(Arc::new(sub));
        }
        let after = entry_sum(&n.entries[i]);
        n.sum = n.sum.wrapping_sub(before).wrapping_add(after);
        return None;
    }
    let old = match &mut n.entries[i] {
        Entry::Leaf(_, _, ev) => Some(core::mem::replace(ev, v)),
        Entry::Sub(s) => insert(Arc::make_mut(s), level + 1, h, k, v),
        Entry::Collide(xs) => {
            if let Some(e) = xs.iter_mut().find(|(eh, ek, _)| *eh == h && *ek == k) {
                Some(core::mem::replace(&mut e.2, v))
            } else if xs.first().is_some_and(|e| low(e.0) == low(h)) {
                xs.push((h, k, v));
                None
            } else {
                // A different 64-bit hash landed here: push the group down.
                let group = core::mem::take(xs);
                let mut sub = HNode::empty();
                for (eh, ek, ev) in group {
                    insert(&mut sub, level + 1, eh, ek, ev);
                }
                insert(&mut sub, level + 1, h, k, v);
                n.entries[i] = Entry::Sub(Arc::new(sub));
                None
            }
        }
    };
    let after = entry_sum(&n.entries[i]);
    n.sum = n.sum.wrapping_sub(before).wrapping_add(after);
    old
}

fn entry_sum<K, V: Value>(e: &Entry<K, V>) -> u128 {
    match e {
        Entry::Leaf(h, _, v) => contribution(*h, v.version()),
        Entry::Sub(s) => s.sum,
        Entry::Collide(xs) => xs.iter().fold(0u128, |a, (h, _, v)| {
            a.wrapping_add(contribution(*h, v.version()))
        }),
    }
}

fn remove<K: Clone + Eq, V: Value>(n: &mut HNode<K, V>, level: u32, h: u128, k: &K) -> Option<V> {
    let bit = 1u32 << slot(h, level);
    if n.bitmap & bit == 0 {
        return None;
    }
    let i = (n.bitmap & (bit - 1)).count_ones() as usize;
    let before = entry_sum(&n.entries[i]);
    let (old, empty) = match &mut n.entries[i] {
        Entry::Leaf(eh, ek, _) => {
            if *eh == h && ek == k {
                let Entry::Leaf(_, _, v) = n.entries.remove(i) else {
                    unreachable!()
                };
                n.bitmap &= !bit;
                n.sum = n.sum.wrapping_sub(before);
                return Some(v);
            }
            (None, false)
        }
        Entry::Sub(s) => {
            let s = Arc::make_mut(s);
            let old = remove(s, level + 1, h, k);
            (old, s.entries.is_empty())
        }
        Entry::Collide(xs) => {
            let old = xs
                .iter()
                .position(|(eh, ek, _)| *eh == h && ek == k)
                .map(|p| xs.remove(p).2);
            (old, xs.is_empty())
        }
    };
    if empty {
        n.entries.remove(i);
        n.bitmap &= !bit;
        n.sum = n.sum.wrapping_sub(before);
    } else {
        let after = entry_sum(&n.entries[i]);
        n.sum = n.sum.wrapping_sub(before).wrapping_add(after);
    }
    old
}

impl<K: Clone + Eq + Hash, V: Value> Value for PMap<K, V> {
    fn version(&self) -> Version {
        PMap::version(self)
    }
}
