//! A transient open-addressing hash table for the runtime's own indexes
//! (the memo, the dedup index, the read and write serials). `no_std`
//! has no `HashMap`; keys hash through the stable hasher.

use alloc::vec::Vec;
use core::hash::Hash;

use crate::hash::{Version, hash64};

/// Keys of a [`Table`]: a 64-bit hash and equality.
pub trait TKey: Eq {
    fn h(&self) -> u64;
}

impl TKey for Version {
    #[inline]
    fn h(&self) -> u64 {
        self.low()
    }
}

/// Any stably hashable key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ByHash<K>(pub K);

impl<K: Hash + Eq> TKey for ByHash<K> {
    #[inline]
    fn h(&self) -> u64 {
        hash64(&self.0)
    }
}

/// Linear probing, power-of-two capacity, load at most 1/2.
pub struct Table<K, V> {
    slots: Vec<Option<(u64, K, V)>>,
    len: usize,
}

impl<K: TKey, V> Default for Table<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: TKey, V> Table<K, V> {
    #[must_use]
    pub fn new() -> Self {
        Table {
            slots: Vec::new(),
            len: 0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        if self.len > 0 {
            self.slots.iter_mut().for_each(|s| *s = None);
            self.len = 0;
        }
    }

    #[inline]
    fn find(&self, h: u64, k: &K) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        // Truncation intended: the probe start.
        #[allow(clippy::cast_possible_truncation)]
        let mut i = (h as usize) & mask;
        loop {
            match &self.slots[i] {
                None => return None,
                Some((sh, sk, _)) if *sh == h && sk == k => return Some(i),
                Some(_) => i = (i + 1) & mask,
            }
        }
    }

    /// The value whose key has hash `h` and satisfies `eq`.
    #[inline]
    pub fn get_by(&self, h: u64, eq: impl Fn(&K) -> bool) -> Option<&V> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        // Truncation intended: the probe start.
        #[allow(clippy::cast_possible_truncation)]
        let mut i = (h as usize) & mask;
        loop {
            match &self.slots[i] {
                None => return None,
                Some((sh, sk, v)) if *sh == h && eq(sk) => return Some(v),
                Some(_) => i = (i + 1) & mask,
            }
        }
    }

    /// [`Table::get_by`], mutable.
    #[inline]
    pub fn get_mut_by(&mut self, h: u64, eq: impl Fn(&K) -> bool) -> Option<&mut V> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        // Truncation intended: the probe start.
        #[allow(clippy::cast_possible_truncation)]
        let mut i = (h as usize) & mask;
        loop {
            match &self.slots[i] {
                None => return None,
                Some((sh, sk, _)) if *sh == h && eq(sk) => {
                    return self.slots[i].as_mut().map(|s| &mut s.2);
                }
                Some(_) => i = (i + 1) & mask,
            }
        }
    }

    /// Set the value at the key with hash `h` satisfying `eq` to `v`,
    /// making the key with `make` if absent; returns the previous value.
    pub fn swap_by(
        &mut self,
        h: u64,
        eq: impl Fn(&K) -> bool,
        make: impl FnOnce() -> K,
        v: V,
    ) -> Option<V> {
        if !self.slots.is_empty() {
            let mask = self.slots.len() - 1;
            // Truncation intended: the probe start.
            #[allow(clippy::cast_possible_truncation)]
            let mut i = (h as usize) & mask;
            loop {
                match &mut self.slots[i] {
                    None => break,
                    Some((sh, sk, sv)) if *sh == h && eq(sk) => {
                        return Some(core::mem::replace(sv, v));
                    }
                    Some(_) => i = (i + 1) & mask,
                }
            }
        }
        if (self.len + 1) * 2 > self.slots.len() {
            self.grow();
        }
        self.put(h, make(), v);
        None
    }

    #[inline]
    #[must_use]
    pub fn get(&self, k: &K) -> Option<&V> {
        let h = k.h();
        self.find(h, k)
            .map(|i| &self.slots[i].as_ref().expect("found").2)
    }

    #[inline]
    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let h = k.h();
        self.find(h, k)
            .map(|i| &mut self.slots[i].as_mut().expect("found").2)
    }

    fn grow(&mut self) {
        let cap = (self.slots.len() * 2).max(16);
        let old = core::mem::replace(&mut self.slots, (0..cap).map(|_| None).collect());
        self.len = 0;
        for (h, k, v) in old.into_iter().flatten() {
            self.put(h, k, v);
        }
    }

    fn put(&mut self, h: u64, k: K, v: V) -> &mut V {
        let mask = self.slots.len() - 1;
        // Truncation intended: the probe start.
        #[allow(clippy::cast_possible_truncation)]
        let mut i = (h as usize) & mask;
        while self.slots[i].is_some() {
            i = (i + 1) & mask;
        }
        self.len += 1;
        &mut self.slots[i].insert((h, k, v)).2
    }

    /// The value at the key with hash `h` satisfying `eq`, inserting
    /// `make()` bound to `init` if absent.
    pub fn entry_by(
        &mut self,
        h: u64,
        eq: impl Fn(&K) -> bool,
        make: impl FnOnce() -> K,
        init: V,
    ) -> &mut V {
        if !self.slots.is_empty() {
            let mask = self.slots.len() - 1;
            // Truncation intended: the probe start.
            #[allow(clippy::cast_possible_truncation)]
            let mut i = (h as usize) & mask;
            loop {
                match &self.slots[i] {
                    None => break,
                    Some((sh, sk, _)) if *sh == h && eq(sk) => {
                        return &mut self.slots[i].as_mut().expect("found").2;
                    }
                    Some(_) => i = (i + 1) & mask,
                }
            }
        }
        if (self.len + 1) * 2 > self.slots.len() {
            self.grow();
        }
        self.put(h, make(), init)
    }

    /// The value at `k`, inserting `f()` if absent.
    pub fn entry(&mut self, k: K, f: impl FnOnce() -> V) -> &mut V {
        let h = k.h();
        if let Some(i) = self.find(h, &k) {
            return &mut self.slots[i].as_mut().expect("found").2;
        }
        if (self.len + 1) * 2 > self.slots.len() {
            self.grow();
        }
        self.put(h, k, f())
    }

    /// Bind `k` to `v`, returning the previous value.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let h = k.h();
        if let Some(i) = self.find(h, &k) {
            return Some(core::mem::replace(
                &mut self.slots[i].as_mut().expect("found").2,
                v,
            ));
        }
        if (self.len + 1) * 2 > self.slots.len() {
            self.grow();
        }
        self.put(h, k, v);
        None
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots.iter().flatten().map(|(_, k, v)| (k, v))
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.slots.iter_mut().flatten().map(|(_, _, v)| v)
    }
}
