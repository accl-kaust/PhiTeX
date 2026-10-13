//! Label-ordered indexes split by key: for every key (a unit and field, a
//! definition site and field, ...) the labels holding an entry, in order.
//!
//! A query names its key and a label range, so it never compares or
//! crosses entries of other keys: a hash lookup, then a search among the
//! key's own labels (a short sorted vector, or a tree once it grows).

use crate::engine::Slot;
use crate::hash::FxHashMap;
use crate::par::Base;
use std::collections::BTreeMap;
use std::collections::btree_map;
use std::hash::Hash;
use std::ops::{Bound, RangeBounds};

/// Entries per key kept in a sorted vector before it becomes a tree.
const SMALL: usize = 24;

enum Bucket {
    Small(Vec<(u64, Slot)>),
    Big(BTreeMap<u64, Slot>),
}

/// An index, or an overlay of one (see `par`): the overlay's `map` holds
/// the entries it set and `tomb` the base entries it removed.
pub(crate) struct LabelIx<K> {
    map: FxHashMap<K, Bucket>,
    base: Base<LabelIx<K>>,
    tomb: FxHashMap<K, Vec<u64>>,
}

impl<K> Default for LabelIx<K> {
    fn default() -> Self {
        LabelIx {
            map: FxHashMap::default(),
            base: Base::none(),
            tomb: FxHashMap::default(),
        }
    }
}

/// The entries of one key in a label range, in label order (both ways).
pub(crate) enum Iter<'a> {
    Empty,
    Small(std::slice::Iter<'a, (u64, Slot)>),
    Big(btree_map::Range<'a, u64, Slot>),
    Merged(Box<Merged<'a>>),
}

/// An overlay's range: its own entries merged with the base's, less the
/// base entries it removed or replaced.
pub(crate) struct Merged<'a> {
    base: Iter<'a>,
    local: Vec<(u64, Slot)>,
    lf: usize,
    lb: usize,
    bf: Option<(u64, Slot)>,
    bb: Option<(u64, Slot)>,
    hidden: Vec<u64>,
}

impl Merged<'_> {
    fn hides(&self, l: u64) -> bool {
        self.hidden.binary_search(&l).is_ok() || self.local.binary_search_by_key(&l, |e| e.0).is_ok()
    }

    fn base_front(&mut self) -> Option<(u64, Slot)> {
        loop {
            if self.bf.is_none() {
                self.bf = self.base.next().or_else(|| self.bb.take());
            }
            let e = self.bf?;
            if !self.hides(e.0) {
                return Some(e);
            }
            self.bf = None;
        }
    }

    fn base_back(&mut self) -> Option<(u64, Slot)> {
        loop {
            if self.bb.is_none() {
                self.bb = self.base.next_back().or_else(|| self.bf.take());
            }
            let e = self.bb?;
            if !self.hides(e.0) {
                return Some(e);
            }
            self.bb = None;
        }
    }
}

impl Iterator for Iter<'_> {
    type Item = (u64, Slot);

    #[inline]
    fn next(&mut self) -> Option<(u64, Slot)> {
        match self {
            Iter::Empty => None,
            Iter::Small(i) => i.next().copied(),
            Iter::Big(i) => i.next().map(|(&l, &s)| (l, s)),
            Iter::Merged(m) => {
                let b = m.base_front();
                let l = (m.lf < m.lb).then(|| m.local[m.lf]);
                match (b, l) {
                    (Some(b), Some(l)) if b.0 < l.0 => {
                        m.bf = None;
                        Some(b)
                    }
                    (_, Some(l)) => {
                        m.lf += 1;
                        Some(l)
                    }
                    (Some(b), None) => {
                        m.bf = None;
                        Some(b)
                    }
                    (None, None) => None,
                }
            }
        }
    }
}

impl DoubleEndedIterator for Iter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<(u64, Slot)> {
        match self {
            Iter::Empty => None,
            Iter::Small(i) => i.next_back().copied(),
            Iter::Big(i) => i.next_back().map(|(&l, &s)| (l, s)),
            Iter::Merged(m) => {
                let b = m.base_back();
                let l = (m.lf < m.lb).then(|| m.local[m.lb - 1]);
                match (b, l) {
                    (Some(b), Some(l)) if b.0 > l.0 => {
                        m.bb = None;
                        Some(b)
                    }
                    (_, Some(l)) => {
                        m.lb -= 1;
                        Some(l)
                    }
                    (Some(b), None) => {
                        m.bb = None;
                        Some(b)
                    }
                    (None, None) => None,
                }
            }
        }
    }
}

/// The positions `[from, to)` of a sorted vector's labels in `r`.
fn span(v: &[(u64, Slot)], r: &impl RangeBounds<u64>) -> (usize, usize) {
    let from = match r.start_bound() {
        Bound::Included(&a) => v.partition_point(|e| e.0 < a),
        Bound::Excluded(&a) => v.partition_point(|e| e.0 <= a),
        Bound::Unbounded => 0,
    };
    let to = match r.end_bound() {
        Bound::Included(&b) => v.partition_point(|e| e.0 <= b),
        Bound::Excluded(&b) => v.partition_point(|e| e.0 < b),
        Bound::Unbounded => v.len(),
    };
    (from, to.max(from))
}

/// Whether `r` holds no label (a `BTreeMap` range would panic on it).
fn empty_range(r: &impl RangeBounds<u64>) -> bool {
    let lo = match r.start_bound() {
        Bound::Included(&a) => Some(a),
        Bound::Excluded(&a) => a.checked_add(1),
        Bound::Unbounded => Some(0),
    };
    let Some(lo) = lo else { return true };
    match r.end_bound() {
        Bound::Included(&b) => lo > b,
        Bound::Excluded(&b) => lo >= b,
        Bound::Unbounded => false,
    }
}

impl<K: Hash + Eq + Copy> LabelIx<K> {
    /// An empty overlay of `base` (itself a plain index).
    pub(crate) fn over(base: &LabelIx<K>) -> Self {
        debug_assert!(base.base.is_none());
        LabelIx {
            map: FxHashMap::default(),
            base: Base::new(base),
            tomb: FxHashMap::default(),
        }
    }

    /// Writes an overlay's changes to its base.
    pub(crate) fn merge_into(self, base: &mut LabelIx<K>) {
        for (k, ls) in self.tomb {
            for l in ls {
                base.remove(&k, l);
            }
        }
        for (k, b) in self.map {
            match b {
                Bucket::Small(v) => {
                    for (l, s) in v {
                        base.insert(k, l, s);
                    }
                }
                Bucket::Big(m) => {
                    for (l, s) in m {
                        base.insert(k, l, s);
                    }
                }
            }
        }
    }

    /// Sets the entry at `label` of `key`; returns the one it replaced.
    pub(crate) fn insert(&mut self, key: K, label: u64, s: Slot) -> Option<Slot> {
        if !self.base.is_none() {
            let old = self.local_insert(key, label, s);
            if old.is_some() {
                return old;
            }
            if self.tomb_del(&key, label) {
                return None;
            }
            return self.base.get().and_then(|b| b.get(&key, label));
        }
        self.local_insert(key, label, s)
    }

    /// Hides the base entry at `label` of `key`; false if already hidden.
    fn tomb_add(&mut self, key: K, label: u64) -> bool {
        let t = self.tomb.entry(key).or_default();
        match t.binary_search(&label) {
            Ok(_) => false,
            Err(i) => {
                t.insert(i, label);
                true
            }
        }
    }

    /// Shows the base entry at `label` of `key` again; whether it was hidden.
    fn tomb_del(&mut self, key: &K, label: u64) -> bool {
        let Some(t) = self.tomb.get_mut(key) else { return false };
        match t.binary_search(&label) {
            Ok(i) => {
                t.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    fn local_insert(&mut self, key: K, label: u64, s: Slot) -> Option<Slot> {
        let b = self.map.entry(key).or_insert_with(|| Bucket::Small(Vec::new()));
        match b {
            Bucket::Small(v) => {
                let i = v.partition_point(|e| e.0 < label);
                if i < v.len() && v[i].0 == label {
                    return Some(std::mem::replace(&mut v[i].1, s));
                }
                if v.len() < SMALL {
                    v.insert(i, (label, s));
                    return None;
                }
                let mut m: BTreeMap<u64, Slot> = v.drain(..).collect();
                m.insert(label, s);
                *b = Bucket::Big(m);
                None
            }
            Bucket::Big(m) => m.insert(label, s),
        }
    }

    /// Removes the entry at `label` of `key`.
    pub(crate) fn remove(&mut self, key: &K, label: u64) -> Option<Slot> {
        if !self.base.is_none() {
            let local = self.local_remove(key, label);
            let was_hidden = !self.tomb_add(*key, label);
            if local.is_some() || was_hidden {
                return local;
            }
            return self.base.get().and_then(|b| b.get(key, label));
        }
        self.local_remove(key, label)
    }

    fn local_remove(&mut self, key: &K, label: u64) -> Option<Slot> {
        let b = self.map.get_mut(key)?;
        let (out, empty) = match b {
            Bucket::Small(v) => {
                let i = v.partition_point(|e| e.0 < label);
                if i < v.len() && v[i].0 == label {
                    let s = v.remove(i).1;
                    (Some(s), v.is_empty())
                } else {
                    (None, false)
                }
            }
            Bucket::Big(m) => {
                let s = m.remove(&label);
                (s, m.is_empty())
            }
        };
        if empty {
            self.map.remove(key);
        }
        out
    }

    pub(crate) fn get(&self, key: &K, label: u64) -> Option<Slot> {
        if let Some(b) = self.base.get() {
            if let Some(s) = self.local_get(key, label) {
                return Some(s);
            }
            if self.tomb.get(key).is_some_and(|t| t.binary_search(&label).is_ok()) {
                return None;
            }
            return b.get(key, label);
        }
        self.local_get(key, label)
    }

    fn local_get(&self, key: &K, label: u64) -> Option<Slot> {
        match self.map.get(key)? {
            Bucket::Small(v) => {
                let i = v.partition_point(|e| e.0 < label);
                (i < v.len() && v[i].0 == label).then(|| v[i].1)
            }
            Bucket::Big(m) => m.get(&label).copied(),
        }
    }

    /// The entries of `key` with labels in `r`, in label order.
    pub(crate) fn range(&self, key: &K, r: impl RangeBounds<u64> + Clone) -> Iter<'_> {
        if empty_range(&r) {
            return Iter::Empty;
        }
        if let Some(b) = self.base.get() {
            let local: Vec<(u64, Slot)> = self.local_range(key, r.clone()).collect();
            let hidden: Vec<u64> = self
                .tomb
                .get(key)
                .map_or_else(Vec::new, |t| t.iter().copied().filter(|l| r.contains(l)).collect());
            if local.is_empty() && hidden.is_empty() {
                return b.range(key, r);
            }
            let lb = local.len();
            return Iter::Merged(Box::new(Merged {
                base: b.range(key, r),
                local,
                lf: 0,
                lb,
                bf: None,
                bb: None,
                hidden,
            }));
        }
        self.local_range(key, r)
    }

    fn local_range(&self, key: &K, r: impl RangeBounds<u64>) -> Iter<'_> {
        match self.map.get(key) {
            None => Iter::Empty,
            Some(Bucket::Small(v)) => {
                let (a, b) = span(v, &r);
                Iter::Small(v[a..b].iter())
            }
            Some(Bucket::Big(m)) => Iter::Big(m.range(r)),
        }
    }
}

/// A map from node slots to values, stored densely by slot index and
/// tagged with the slot's generation: a slot of a freed node (an older
/// generation) finds nothing, like a hash map keyed by the slot.
/// An overlay (see `par`) keeps its own values in `ov` (`None`: removed).
pub(crate) struct SlotMap<V> {
    vals: Vec<Option<(u32, V)>>,
    base: Base<SlotMap<V>>,
    ov: FxHashMap<Slot, Option<V>>,
}

impl<V> Default for SlotMap<V> {
    fn default() -> Self {
        SlotMap {
            vals: Vec::new(),
            base: Base::none(),
            ov: FxHashMap::default(),
        }
    }
}

impl<V: Clone> SlotMap<V> {
    pub(crate) fn over(base: &SlotMap<V>) -> Self {
        SlotMap {
            vals: Vec::new(),
            base: Base::new(base),
            ov: FxHashMap::default(),
        }
    }

    pub(crate) fn merge_into(self, base: &mut SlotMap<V>) {
        for (s, v) in self.ov {
            match v {
                Some(v) => base.insert(s, v),
                None => base.remove(s),
            }
        }
    }

    /// Every (slot, value), by slot index (a plain map only).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (Slot, &V)> {
        debug_assert!(self.base.is_none(), "whole-map access to an overlay");
        self.vals.iter().enumerate().filter_map(|(i, e)| {
            e.as_ref().map(|(g, v)| {
                (
                    Slot {
                        ix: u32::try_from(i).expect("slot index"),
                        gen_: *g,
                    },
                    v,
                )
            })
        })
    }

    fn plain_get(&self, s: Slot) -> Option<&V> {
        match self.vals.get(s.ix as usize) {
            Some(Some((g, v))) if *g == s.gen_ => Some(v),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn get(&self, s: Slot) -> Option<&V> {
        if let Some(b) = self.base.get() {
            return match self.ov.get(&s) {
                Some(v) => v.as_ref(),
                None => b.plain_get(s),
            };
        }
        self.plain_get(s)
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, s: Slot) -> Option<&mut V> {
        if let Some(b) = self.base.get() {
            return match self.ov.entry(s) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut().as_mut(),
                std::collections::hash_map::Entry::Vacant(e) => e.insert(Some(b.plain_get(s)?.clone())).as_mut(),
            };
        }
        match self.vals.get_mut(s.ix as usize) {
            Some(Some((g, v))) if *g == s.gen_ => Some(v),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn insert(&mut self, s: Slot, v: V) {
        if !self.base.is_none() {
            self.ov.insert(s, Some(v));
            return;
        }
        let i = s.ix as usize;
        if self.vals.len() <= i {
            self.vals.resize_with(i + 1, || None);
        }
        self.vals[i] = Some((s.gen_, v));
    }

    #[inline]
    pub(crate) fn remove(&mut self, s: Slot) {
        if !self.base.is_none() {
            self.ov.insert(s, None);
            return;
        }
        if let Some(e) = self.vals.get_mut(s.ix as usize)
            && e.as_ref().is_some_and(|(g, _)| *g == s.gen_)
        {
            *e = None;
        }
    }

    /// The value at `s`, made with `make` if there is none.
    pub(crate) fn entry_or(&mut self, s: Slot, make: impl FnOnce() -> V) -> &mut V {
        if self.get(s).is_none() {
            self.insert(s, make());
        }
        self.get_mut(s).expect("just inserted")
    }
}

/// Per node slot, a count that a `clear` resets for every slot at once
/// (each count carries the clear epoch it was made in). An overlay (see
/// `par`) counts in `ov` on top of its base's counts.
pub(crate) struct SlotCounts {
    vals: Vec<(u32, u32, u32)>,
    epoch: u32,
    base: Base<SlotCounts>,
    ov: FxHashMap<Slot, u32>,
}

impl Default for SlotCounts {
    fn default() -> Self {
        SlotCounts {
            vals: Vec::new(),
            epoch: 1,
            base: Base::none(),
            ov: FxHashMap::default(),
        }
    }
}

impl SlotCounts {
    pub(crate) fn over(base: &SlotCounts) -> Self {
        SlotCounts {
            base: Base::new(base),
            ..SlotCounts::default()
        }
    }

    pub(crate) fn merge_into(self, base: &mut SlotCounts) {
        for (s, n) in self.ov {
            let c = base.get(s).unwrap_or(0) + n;
            base.set(s, c);
        }
    }

    fn plain_get(&self, s: Slot) -> Option<u32> {
        match self.vals.get(s.ix as usize) {
            Some(&(e, g, c)) if e == self.epoch && g == s.gen_ => Some(c),
            _ => None,
        }
    }

    pub(crate) fn get(&self, s: Slot) -> Option<u32> {
        if let Some(b) = self.base.get() {
            let d = self.ov.get(&s).copied();
            return match (b.plain_get(s), d) {
                (None, None) => None,
                (c, d) => Some(c.unwrap_or(0) + d.unwrap_or(0)),
            };
        }
        self.plain_get(s)
    }

    fn set(&mut self, s: Slot, c: u32) {
        let i = s.ix as usize;
        if self.vals.len() <= i {
            self.vals.resize(i + 1, (0, 0, 0));
        }
        self.vals[i] = (self.epoch, s.gen_, c);
    }

    /// Adds one to the count of `s`; returns the new count.
    pub(crate) fn bump(&mut self, s: Slot) -> u32 {
        if !self.base.is_none() {
            *self.ov.entry(s).or_insert(0) += 1;
            return self.get(s).unwrap_or(0);
        }
        let c = self.get(s).unwrap_or(0) + 1;
        self.set(s, c);
        c
    }

    pub(crate) fn clear(&mut self) {
        debug_assert!(self.base.is_none());
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.vals.clear();
            self.epoch = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LabelIx;

    #[test]
    fn an_overlay_matches_its_base_with_its_changes() {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        for round in 0..40u64 {
            let mut base: LabelIx<u32> = LabelIx::default();
            let mut m: BTreeMap<(u32, u64), Slot> = BTreeMap::new();
            let span = if round % 2 == 0 { 40 } else { 400 };
            for _ in 0..rnd(300) {
                let k = u32::try_from(rnd(3)).unwrap();
                let (l, s) = (rnd(span), Slot::unpack(rnd(1 << 20)));
                base.insert(k, l, s);
                m.insert((k, l), s);
            }
            let mut ov = LabelIx::over(&base);
            for _ in 0..400 {
                let k = u32::try_from(rnd(3)).unwrap();
                let l = rnd(span);
                if rnd(2) == 0 {
                    let s = Slot::unpack(rnd(1 << 20));
                    assert_eq!(ov.insert(k, l, s), m.insert((k, l), s));
                } else {
                    assert_eq!(ov.remove(&k, l), m.remove(&(k, l)));
                }
                assert_eq!(ov.get(&k, l), m.get(&(k, l)).copied());
                let (a, b) = (rnd(span + 10), rnd(span + 10));
                let want: Vec<(u64, Slot)> = if a < b {
                    m.range((k, a)..(k, b)).map(|(kk, &v)| (kk.1, v)).collect()
                } else {
                    Vec::new()
                };
                assert_eq!(ov.range(&k, a..b).collect::<Vec<_>>(), want);
                let back: Vec<(u64, Slot)> = ov.range(&k, a..b).rev().collect();
                assert_eq!(back, want.iter().rev().copied().collect::<Vec<_>>());
                // From both ends at once.
                let mut it = ov.range(&k, a..b);
                let (mut f, mut r) = (Vec::new(), Vec::new());
                loop {
                    let e = if rnd(2) == 0 {
                        it.next().map(|e| f.push(e))
                    } else {
                        it.next_back().map(|e| r.push(e))
                    };
                    if e.is_none() {
                        break;
                    }
                }
                r.reverse();
                f.extend(r);
                assert_eq!(f, want);
            }
            let mut merged = base;
            ov.merge_into(&mut merged);
            for k in 0..3u32 {
                let got: Vec<(u64, Slot)> = merged.range(&k, ..).collect();
                let want: Vec<(u64, Slot)> = m.range((k, 0)..=(k, u64::MAX)).map(|(kk, &v)| (kk.1, v)).collect();
                assert_eq!(got, want);
            }
        }
    }
    use crate::engine::Slot;
    use std::collections::BTreeMap;

    #[test]
    fn matches_a_composite_btree() {
        let mut ix: LabelIx<(u32, u32)> = LabelIx::default();
        let mut m: BTreeMap<(u32, u32, u64), Slot> = BTreeMap::new();
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        for step in 0..60_000u32 {
            let k = (u32::try_from(rnd(3)).unwrap(), u32::try_from(rnd(4)).unwrap());
            let l = rnd(if step % 3 == 0 { 40 } else { 400 });
            let s = Slot::unpack(rnd(1 << 20));
            if rnd(3) > 0 {
                assert_eq!(ix.insert(k, l, s), m.insert((k.0, k.1, l), s));
            } else {
                assert_eq!(ix.remove(&k, l), m.remove(&(k.0, k.1, l)));
            }
            assert_eq!(ix.get(&k, l), m.get(&(k.0, k.1, l)).copied());
            let (a, b) = (rnd(450), rnd(450));
            let got: Vec<(u64, Slot)> = ix.range(&k, a..b).collect();
            let want: Vec<(u64, Slot)> = if a < b {
                m.range((k.0, k.1, a)..(k.0, k.1, b))
                    .map(|(kk, &v)| (kk.2, v))
                    .collect()
            } else {
                Vec::new()
            };
            assert_eq!(got, want);
            let back = ix.range(&k, ..b).next_back();
            let want = m
                .range((k.0, k.1, 0)..(k.0, k.1, b))
                .next_back()
                .map(|(kk, &v)| (kk.2, v));
            assert_eq!(back, want);
            let fwd = ix.range(&k, a..=b).next();
            let want = if a <= b {
                m.range((k.0, k.1, a)..=(k.0, k.1, b)).next().map(|(kk, &v)| (kk.2, v))
            } else {
                None
            };
            assert_eq!(fwd, want);
        }
    }
}
