//! A map by name kept as a shared base and a small top of changes: a
//! copy shares the base and copies the top alone, so a chain of
//! definitions, each the one before plus a chunk's, holds one copy of
//! what they share (`phitex-tex`'s `Incr::chain` kept a full copy per
//! chunk that defined anything: 120 MB in the `TikZ` manual). The top
//! goes into a new base once it holds [`TOP`] names.

use std::sync::Arc;

type HashMap<K, V> =
    std::collections::HashMap<K, V, std::hash::BuildHasherDefault<crate::pass::hints::Fx>>;

/// Names the top holds before it goes into a new base.
const TOP: usize = 256;

#[derive(Debug)]
pub(crate) struct Layered<V> {
    base: Arc<HashMap<String, V>>,
    /// Names changed since the base: their value, or none for a removal.
    top: HashMap<String, Option<V>>,
}

impl<V> Default for Layered<V> {
    fn default() -> Self {
        Layered {
            base: Arc::new(HashMap::default()),
            top: HashMap::default(),
        }
    }
}

impl<V: Clone> Clone for Layered<V> {
    fn clone(&self) -> Self {
        Layered {
            base: self.base.clone(),
            top: self.top.clone(),
        }
    }
}

impl<V: Clone> From<HashMap<String, V>> for Layered<V> {
    fn from(m: HashMap<String, V>) -> Self {
        Layered {
            base: Arc::new(m),
            top: HashMap::default(),
        }
    }
}

impl<V: Clone> Layered<V> {
    pub(crate) fn get(&self, k: &str) -> Option<&V> {
        match self.top.get(k) {
            Some(x) => x.as_ref(),
            None => self.base.get(k),
        }
    }

    pub(crate) fn contains_key(&self, k: &str) -> bool {
        self.get(k).is_some()
    }

    pub(crate) fn len(&self) -> usize {
        let mut n = self.base.len();
        for (k, v) in &self.top {
            match (self.base.contains_key(k), v.is_some()) {
                (false, true) => n += 1,
                (true, false) => n -= 1,
                _ => {}
            }
        }
        n
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Its names and values, in no order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.top
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| (k, v)))
            .chain(
                self.base
                    .iter()
                    .filter(|(k, _)| !self.top.contains_key(k.as_str())),
            )
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &String> {
        self.iter().map(|(k, _)| k)
    }

    /// The value of `k` to change, its default made where it has none.
    pub(crate) fn entry_or_default(&mut self, k: &str) -> &mut V
    where
        V: Default,
    {
        self.settle();
        if !self.top.contains_key(k) {
            let v = self.base.get(k).cloned();
            self.top.insert(k.to_owned(), v);
        }
        self.top
            .get_mut(k)
            .expect("just put")
            .get_or_insert_with(V::default)
    }

    pub(crate) fn insert(&mut self, k: String, v: V) {
        self.settle();
        self.top.insert(k, Some(v));
    }

    pub(crate) fn remove(&mut self, k: &str) {
        if self.contains_key(k) {
            self.settle();
            self.top.insert(k.to_owned(), None);
        }
    }

    /// As one map (a store's coding).
    pub(crate) fn to_map(&self) -> HashMap<String, V> {
        self.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// The top into a new base, once it holds [`TOP`] names.
    fn settle(&mut self) {
        if self.top.len() < TOP {
            return;
        }
        let mut m = (*self.base).clone();
        for (k, v) in self.top.drain() {
            match v {
                Some(v) => {
                    m.insert(k, v);
                }
                None => {
                    m.remove(&k);
                }
            }
        }
        self.base = Arc::new(m);
    }
}

/// A set of names kept as [`Layered`] is.
pub(crate) type LayeredSet = Layered<()>;

impl LayeredSet {
    pub(crate) fn contains(&self, k: &str) -> bool {
        self.contains_key(k)
    }

    pub(crate) fn insert_name(&mut self, k: String) {
        self.insert(k, ());
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &String> {
        self.keys()
    }

    pub(crate) fn from_set(
        s: std::collections::HashSet<String, std::hash::BuildHasherDefault<crate::pass::hints::Fx>>,
    ) -> Self {
        Layered::from(
            s.into_iter()
                .map(|k| (k, ()))
                .collect::<HashMap<String, ()>>(),
        )
    }

    pub(crate) fn to_set(
        &self,
    ) -> std::collections::HashSet<String, std::hash::BuildHasherDefault<crate::pass::hints::Fx>>
    {
        self.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layered_map_reads_as_the_map_it_stands_for_through_copies_and_settles() {
        let mut a: Layered<Vec<u32>> = Layered::default();
        let mut want: HashMap<String, Vec<u32>> = HashMap::default();
        let mut copies = Vec::new();
        for i in 0..2000u32 {
            let k = format!("n{}", i % 700);
            if i % 11 == 0 {
                a.remove(&k);
                want.remove(&k);
            } else {
                a.entry_or_default(&k).push(i);
                want.entry(k.clone()).or_default().push(i);
            }
            if i % 97 == 0 {
                copies.push((a.clone(), want.clone()));
            }
        }
        copies.push((a, want));
        for (l, m) in &copies {
            assert_eq!(l.len(), m.len());
            assert_eq!(&l.to_map(), m);
            for (k, v) in m {
                assert_eq!(l.get(k), Some(v));
            }
            assert!(l.get("absent").is_none());
        }
    }
}
