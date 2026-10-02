//! A small open-addressing hash map from `u64` keys (already well mixed
//! hashes) to copyable values: the memo's tables, where `BTreeMap`'s
//! comparisons showed in profiles (DESIGN.md §7.7).

use alloc::vec::Vec;

/// Slot states: never used, removed (a probe goes on past it), in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Empty,
    Gone,
    Full,
}

partex_engine::persist_enum!(State { Empty, Gone, Full });

#[derive(Clone, Debug)]
pub(crate) struct U64Map<V: Copy + Default> {
    keys: Vec<u64>,
    vals: Vec<V>,
    state: Vec<State>,
    /// Slots in use, and in use or removed.
    len: usize,
    used: usize,
}

impl<V: Copy + Default + partex_engine::persist::Persist> partex_engine::persist::Persist
    for U64Map<V>
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        let Self {
            keys,
            vals,
            state,
            len,
            used,
        } = self;
        keys.save(s);
        vals.save(s);
        state.save(s);
        len.save(s);
        used.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        use partex_engine::persist::Persist;
        Some(Self {
            keys: Persist::load(l)?,
            vals: Persist::load(l)?,
            state: Persist::load(l)?,
            len: Persist::load(l)?,
            used: Persist::load(l)?,
        })
    }
}

impl<V: Copy + Default> Default for U64Map<V> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            vals: Vec::new(),
            state: Vec::new(),
            len: 0,
            used: 0,
        }
    }
}

impl<V: Copy + Default> U64Map<V> {
    fn mask(&self) -> usize {
        self.keys.len() - 1
    }

    #[inline]
    fn home(&self, k: u64) -> usize {
        // (the high bits of a multiplicative mix)
        let h = k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (h >> 32) as usize & self.mask()
    }

    /// The value under `k`.
    #[inline]
    pub(crate) fn get(&self, k: u64) -> Option<V> {
        if self.keys.is_empty() {
            return None;
        }
        let mut i = self.home(k);
        loop {
            match self.state[i] {
                State::Empty => return None,
                State::Full if self.keys[i] == k => return Some(self.vals[i]),
                _ => i = (i + 1) & self.mask(),
            }
        }
    }

    /// Set the value under `k`.
    pub(crate) fn insert(&mut self, k: u64, v: V) {
        if (self.used + 1) * 4 >= self.keys.len() * 3 {
            self.grow();
        }
        let mut i = self.home(k);
        let mut gone = None;
        loop {
            match self.state[i] {
                State::Empty => break,
                State::Gone => {
                    gone.get_or_insert(i);
                }
                State::Full if self.keys[i] == k => {
                    self.vals[i] = v;
                    return;
                }
                State::Full => {}
            }
            i = (i + 1) & self.mask();
        }
        let i = gone.unwrap_or_else(|| {
            self.used += 1;
            i
        });
        self.keys[i] = k;
        self.vals[i] = v;
        self.state[i] = State::Full;
        self.len += 1;
    }

    /// Remove `k`, if present.
    pub(crate) fn remove(&mut self, k: u64) {
        if self.keys.is_empty() {
            return;
        }
        let mut i = self.home(k);
        loop {
            match self.state[i] {
                State::Empty => return,
                State::Full if self.keys[i] == k => {
                    self.state[i] = State::Gone;
                    self.len -= 1;
                    return;
                }
                _ => i = (i + 1) & self.mask(),
            }
        }
    }

    fn grow(&mut self) {
        let n = (self.len * 2).max(8).next_power_of_two() * 2;
        let old = core::mem::replace(
            self,
            Self {
                keys: alloc::vec![0; n],
                vals: alloc::vec![V::default(); n],
                state: alloc::vec![State::Empty; n],
                len: 0,
                used: 0,
            },
        );
        for i in 0..old.keys.len() {
            if old.state[i] == State::Full {
                self.insert(old.keys[i], old.vals[i]);
            }
        }
    }
}

/// Equal when the same keys hold equal values.
impl<V: Copy + Default + PartialEq> PartialEq for U64Map<V> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && (0..self.keys.len())
                .filter(|&i| self.state[i] == State::Full)
                .all(|i| other.get(self.keys[i]) == Some(self.vals[i]))
    }
}

impl<V: Copy + Default + Eq> Eq for U64Map<V> {}

#[cfg(test)]
mod tests {
    use super::U64Map;

    #[test]
    fn insert_get_remove() {
        let mut m: U64Map<i32> = U64Map::default();
        for k in 0..1000u64 {
            m.insert(k * 7919, i32::try_from(k).unwrap());
        }
        for k in (0..1000u64).step_by(2) {
            m.remove(k * 7919);
        }
        for k in 0..1000u64 {
            let want = (k % 2 == 1).then(|| i32::try_from(k).unwrap());
            assert_eq!(m.get(k * 7919), want);
        }
        m.insert(7919 * 2, 5);
        assert_eq!(m.get(7919 * 2), Some(5));
    }
}
