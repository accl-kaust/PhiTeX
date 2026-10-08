//! The memo store (DESIGN 7.8): results of expensive ops by content key,
//! valid across builds and documents, with a byte budget, a pluggable
//! eviction policy and a byte image through the client's codec.
//!
//! Keys are content: a leaf's is its op's tag and its operands' versions,
//! a scan element's its op's tag, the state's, the element's and the
//! arguments' versions. Only ops the client opts in (`Lang::memo`) are
//! probed: a probe costs a hash and a lookup.

use crate::graph::Map as HashMap;
use crate::ver::Ver;

/// How a value is written to and read from bytes (for the image).
pub trait Codec<V> {
    fn encode(&self, v: &V, out: &mut Vec<u8>);
    /// The value at the start of `b`, advancing `b` past it.
    fn decode(&self, b: &mut &[u8]) -> Option<V>;
}

/// Which entries go when the store is over its budget.
pub trait Evict: Send {
    /// Entry `k` (of `bytes`) was put or read.
    fn touch(&mut self, k: Ver, bytes: usize);
    /// Entry `k` is gone.
    fn forget(&mut self, k: Ver);
    /// Entries to drop, oldest first, until `excess` bytes are freed.
    fn victims(&mut self, excess: usize) -> Vec<Ver>;
}

/// Least recently used, by bytes. A use only stamps its entry; the
/// order is made when victims are asked for (rarely: the store frees an
/// eighth of its budget beyond what it must each time).
#[derive(Default)]
pub struct Lru {
    clock: u64,
    /// Entry → (last use, bytes).
    at: HashMap<Ver, (u64, usize)>,
}

impl Evict for Lru {
    fn touch(&mut self, k: Ver, bytes: usize) {
        self.clock += 1;
        self.at.insert(k, (self.clock, bytes));
    }
    fn forget(&mut self, k: Ver) {
        self.at.remove(&k);
    }
    fn victims(&mut self, excess: usize) -> Vec<Ver> {
        let mut all: Vec<(u64, usize, Ver)> = self.at.iter().map(|(k, e)| (e.0, e.1, *k)).collect();
        all.sort_unstable_by_key(|e| e.0);
        let mut out = Vec::new();
        let mut freed = 0;
        for (_, b, k) in all {
            if freed >= excess {
                break;
            }
            freed += b;
            out.push(k);
        }
        out
    }
}

/// An error reading an image.
#[derive(Debug, PartialEq, Eq)]
pub enum MemoError {
    /// Not an image, or of another format version.
    Format,
    /// An entry did not decode.
    Decode,
}

/// The store: key → (value, bytes).
pub struct Memo<V, E: Evict = Lru> {
    map: HashMap<Ver, (V, usize)>,
    bytes: usize,
    /// The budget in bytes (0: no store).
    pub budget: usize,
    evict: E,
    /// Probes, and the ones that found an entry.
    pub probes: u64,
    pub hits: u64,
    /// Probes and hits per op (by `Lang::op_tag`), for the profile.
    pub(crate) per_tag: HashMap<u64, (u64, u64)>,
}

impl<V: Clone> Default for Memo<V, Lru> {
    fn default() -> Self {
        Self::new(Lru::default())
    }
}

/// The image's magic and format version.
const MAGIC: &[u8; 8] = b"phimemo1";

impl<V: Clone, E: Evict> Memo<V, E> {
    #[must_use]
    pub fn new(evict: E) -> Self {
        Memo {
            map: HashMap::default(),
            bytes: 0,
            budget: 0,
            evict,
            probes: 0,
            hits: 0,
            per_tag: HashMap::default(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Bytes held (as the entries were put).
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn get(&mut self, k: Ver) -> Option<&V> {
        self.probes += 1;
        let (v, b) = self.map.get(&k)?;
        self.hits += 1;
        self.evict.touch(k, *b);
        Some(v)
    }

    /// A probe for op `tag` counted (a hit or not).
    pub(crate) fn note(&mut self, tag: u64, hit: bool) {
        let e = self.per_tag.entry(tag).or_default();
        e.0 += 1;
        e.1 += u64::from(hit);
    }

    /// Store `v` under `k` (`bytes`: what it holds), dropping the
    /// policy's victims while over budget.
    pub fn put(&mut self, k: Ver, v: V, bytes: usize) {
        if bytes > self.budget {
            return;
        }
        if let Some((_, b)) = self.map.insert(k, (v, bytes)) {
            self.bytes -= b;
        }
        self.bytes += bytes;
        self.evict.touch(k, bytes);
        if self.bytes > self.budget {
            let excess = self.bytes - self.budget + self.budget / 8;
            for x in self.evict.victims(excess) {
                if x == k {
                    continue;
                }
                if let Some((_, b)) = self.map.remove(&x) {
                    self.bytes -= b;
                }
                self.evict.forget(x);
            }
        }
    }

    /// The image: magic, count, then (key, bytes, value) each.
    pub fn save<C: Codec<V>>(&self, c: &C, out: &mut Vec<u8>) {
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.map.len() as u64).to_le_bytes());
        let mut keys: Vec<&Ver> = self.map.keys().collect();
        keys.sort_unstable();
        for k in keys {
            let (v, b) = &self.map[k];
            out.extend_from_slice(&k.0.to_le_bytes());
            out.extend_from_slice(&(*b as u64).to_le_bytes());
            c.encode(v, out);
        }
    }

    /// Entries from an image. On an error the store is left empty (a bad
    /// image drops the memo, never a build).
    ///
    /// # Errors
    ///
    /// A wrong magic or version, or an entry that does not decode.
    pub fn load<C: Codec<V>>(&mut self, c: &C, mut b: &[u8]) -> Result<(), MemoError> {
        let r = self.load_inner(c, &mut b);
        if r.is_err() {
            self.map.clear();
            self.bytes = 0;
        }
        r
    }

    fn load_inner<C: Codec<V>>(&mut self, c: &C, b: &mut &[u8]) -> Result<(), MemoError> {
        fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
            if b.len() < n {
                return None;
            }
            let (h, t) = b.split_at(n);
            *b = t;
            Some(h)
        }
        if take(b, 8) != Some(&MAGIC[..]) {
            return Err(MemoError::Format);
        }
        let n = u64::from_le_bytes(
            take(b, 8)
                .ok_or(MemoError::Format)?
                .try_into()
                .expect("8 bytes"),
        );
        for _ in 0..n {
            let k = u128::from_le_bytes(
                take(b, 16)
                    .ok_or(MemoError::Decode)?
                    .try_into()
                    .expect("16 bytes"),
            );
            let bytes = u64::from_le_bytes(
                take(b, 8)
                    .ok_or(MemoError::Decode)?
                    .try_into()
                    .expect("8 bytes"),
            );
            let v = c.decode(b).ok_or(MemoError::Decode)?;
            self.put(
                Ver(k),
                v,
                usize::try_from(bytes).map_err(|_| MemoError::Decode)?,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct U64;
    impl Codec<u64> for U64 {
        fn encode(&self, v: &u64, out: &mut Vec<u8>) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        fn decode(&self, b: &mut &[u8]) -> Option<u64> {
            let (h, t) = b.split_at_checked(8)?;
            *b = t;
            Some(u64::from_le_bytes(h.try_into().ok()?))
        }
    }

    #[test]
    fn lru_keeps_the_budget_and_the_image_round_trips() {
        let mut m: Memo<u64> = Memo {
            budget: 100,
            ..Memo::default()
        };
        for i in 0..20u64 {
            m.put(Ver(u128::from(i)), i, 10);
        }
        assert!(m.bytes() <= 100);
        assert!((7..=10).contains(&m.len()), "{}", m.len());
        // (the oldest went)
        assert!(m.get(Ver(0)).is_none());
        assert_eq!(m.get(Ver(19)), Some(&19));
        let mut img = Vec::new();
        m.save(&U64, &mut img);
        let mut n: Memo<u64> = Memo {
            budget: 1000,
            ..Memo::default()
        };
        assert_eq!(n.load(&U64, &img), Ok(()));
        assert_eq!(n.len(), m.len());
        assert_eq!(n.get(Ver(15)), Some(&15));
        // a bad image drops the store
        assert_eq!(n.load(&U64, &img[..img.len() - 3]), Err(MemoError::Decode));
        assert!(n.is_empty());
        assert_eq!(n.load(&U64, b"nonsense"), Err(MemoError::Format));
    }
}
