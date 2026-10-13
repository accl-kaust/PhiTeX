//! Maps and sets that can be overlays of a frozen base (see `par`).
//!
//! A plain map (no base) behaves as an `FxHashMap`. An overlay holds the
//! entries its owner wrote and the keys it removed; every read consults
//! them first and the base after. `merge_into` writes them to the base
//! once the base may change again.

use crate::hash::{FxHashMap, FxHashSet};
use crate::par::Base;
use std::hash::Hash;

pub(crate) struct OvMap<K, V> {
    m: FxHashMap<K, V>,
    base: Base<OvMap<K, V>>,
    tomb: FxHashSet<K>,
}

impl<K, V> Default for OvMap<K, V> {
    fn default() -> Self {
        OvMap {
            m: FxHashMap::default(),
            base: Base::none(),
            tomb: FxHashSet::default(),
        }
    }
}

impl<K: Hash + Eq + Clone, V: Clone> OvMap<K, V> {
    /// An empty overlay of `base` (itself a plain map).
    pub(crate) fn over(base: &OvMap<K, V>) -> Self {
        debug_assert!(base.base.is_none());
        OvMap {
            m: FxHashMap::default(),
            base: Base::new(base),
            tomb: FxHashSet::default(),
        }
    }

    /// The map itself, for the operations a plain map has only.
    pub(crate) fn plain(&self) -> &FxHashMap<K, V> {
        debug_assert!(self.base.is_none(), "whole-map access to an overlay");
        &self.m
    }

    pub(crate) fn plain_mut(&mut self) -> &mut FxHashMap<K, V> {
        debug_assert!(self.base.is_none(), "whole-map access to an overlay");
        &mut self.m
    }

    #[inline]
    pub(crate) fn get(&self, k: &K) -> Option<&V> {
        if let Some(v) = self.m.get(k) {
            return Some(v);
        }
        match self.base.get() {
            Some(b) if !self.tomb.contains(k) => b.m.get(k),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn contains_key(&self, k: &K) -> bool {
        self.get(k).is_some()
    }

    /// The number of entries: exact for a plain map; for an overlay, its
    /// base's count plus its own entries (a bound, not a count).
    pub(crate) fn len(&self) -> usize {
        self.m.len() + self.base.get().map_or(0, |b| b.m.len())
    }

    pub(crate) fn into_plain(self) -> FxHashMap<K, V> {
        debug_assert!(self.base.is_none(), "whole-map access to an overlay");
        self.m
    }

    /// Whether the map may be empty: exact for a plain map; for an overlay,
    /// false whenever the base holds anything.
    pub(crate) fn is_empty(&self) -> bool {
        self.m.is_empty() && self.base.get().is_none_or(|b| b.m.is_empty())
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        if !self.base.is_none() && !self.m.contains_key(k) {
            let v = self.base_value(k)?;
            self.m.insert(k.clone(), v);
        }
        self.m.get_mut(k)
    }

    fn base_value(&self, k: &K) -> Option<V> {
        let b = self.base.get()?;
        if self.tomb.contains(k) {
            return None;
        }
        b.m.get(k).cloned()
    }

    #[inline]
    pub(crate) fn insert(&mut self, k: K, v: V) -> Option<V> {
        if self.base.is_none() {
            return self.m.insert(k, v);
        }
        let old = match self.m.get(&k) {
            Some(_) => None,
            None => self.base_value(&k),
        };
        self.tomb.remove(&k);
        self.m.insert(k, v).or(old)
    }

    #[inline]
    pub(crate) fn remove(&mut self, k: &K) -> Option<V> {
        if self.base.is_none() {
            return self.m.remove(k);
        }
        let local = self.m.remove(k);
        let old = if local.is_some() { None } else { self.base_value(k) };
        self.tomb.insert(k.clone());
        local.or(old)
    }

    /// The value at `k`, made with `make` if there is none.
    #[inline]
    pub(crate) fn entry_or(&mut self, k: K, make: impl FnOnce() -> V) -> &mut V {
        if self.base.is_none() {
            return self.m.entry(k).or_insert_with(make);
        }
        if self.get_mut(&k).is_none() {
            self.tomb.remove(&k);
            self.m.insert(k.clone(), make());
        }
        self.m.get_mut(&k).expect("just inserted")
    }

    /// Writes the overlay's changes to `base` (its values replace).
    pub(crate) fn merge_into(self, base: &mut OvMap<K, V>) {
        for k in self.tomb {
            base.m.remove(&k);
        }
        base.m.extend(self.m);
    }

    /// The overlay's own entries (changes relative to the base).
    pub(crate) fn local(&self) -> &FxHashMap<K, V> {
        &self.m
    }
}

pub(crate) struct OvSet<K> {
    m: OvMap<K, ()>,
}

impl<K> Default for OvSet<K> {
    fn default() -> Self {
        OvSet { m: OvMap::default() }
    }
}

impl<K: Hash + Eq + Clone> OvSet<K> {
    pub(crate) fn over(base: &OvSet<K>) -> Self {
        OvSet {
            m: OvMap::over(&base.m),
        }
    }

    #[allow(clippy::zero_sized_map_values)]
    pub(crate) fn plain_mut(&mut self) -> &mut FxHashMap<K, ()> {
        self.m.plain_mut()
    }

    #[inline]
    pub(crate) fn contains(&self, k: &K) -> bool {
        self.m.contains_key(k)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.m.is_empty()
    }

    /// Whether `k` was not in the set.
    #[inline]
    pub(crate) fn insert(&mut self, k: K) -> bool {
        self.m.insert(k, ()).is_none()
    }

    /// Whether `k` was in the set.
    #[inline]
    pub(crate) fn remove(&mut self, k: &K) -> bool {
        self.m.remove(k).is_some()
    }

    pub(crate) fn merge_into(self, base: &mut OvSet<K>) {
        self.m.merge_into(&mut base.m);
    }
}

#[cfg(test)]
mod tests {
    use super::OvMap;
    use std::collections::HashMap;

    #[test]
    fn an_overlay_reads_as_the_base_with_its_changes() {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        for _ in 0..50 {
            let mut base: OvMap<u64, u64> = OvMap::default();
            let mut model: HashMap<u64, u64> = HashMap::new();
            for _ in 0..rnd(60) {
                let (k, v) = (rnd(40), rnd(1000));
                base.insert(k, v);
                model.insert(k, v);
            }
            let mut ov = OvMap::over(&base);
            for _ in 0..200 {
                let k = rnd(50);
                // Keep the model in step with what the overlay holds.
                model.clear();
                for k in 0..50 {
                    if let Some(&v) = ov.get(&k) {
                        model.insert(k, v);
                    }
                }
                match rnd(4) {
                    0 => {
                        let v = rnd(1000);
                        let want = model.insert(k, v);
                        assert_eq!(ov.insert(k, v), want);
                    }
                    1 => {
                        let want = model.remove(&k);
                        assert_eq!(ov.remove(&k), want);
                    }
                    2 => {
                        if let Some(v) = ov.get_mut(&k) {
                            *v += 1;
                            *model.get_mut(&k).expect("model") += 1;
                        } else {
                            assert!(!model.contains_key(&k));
                        }
                    }
                    _ => {
                        *ov.entry_or(k, || 7) += 1;
                        *model.entry(k).or_insert(7) += 1;
                    }
                }
                for k in 0..50 {
                    assert_eq!(ov.get(&k), model.get(&k));
                }
            }
            let mut merged = base;
            ov.merge_into(&mut merged);
            for k in 0..50 {
                assert_eq!(merged.get(&k), model.get(&k));
            }
        }
    }
}
