//! Content hashes: deterministic across runs, threads and platforms. A
//! version is a hash of a value's content and nothing else.

use std::hash::{Hash, Hasher};

/// FxHash-style hasher, word at a time.
#[derive(Default)]
pub struct Fx(pub u64);

impl Fx {
    #[inline]
    fn add(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        // (a final mix so that nearby inputs spread over all bits)
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x
    }
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..c.len()].copy_from_slice(c);
            self.add(u64::from_le_bytes(b));
        }
        self.add(bytes.len() as u64);
    }
    fn write_u8(&mut self, x: u8) {
        self.add(u64::from(x));
    }
    fn write_u16(&mut self, x: u16) {
        self.add(u64::from(x));
    }
    fn write_u32(&mut self, x: u32) {
        self.add(u64::from(x));
    }
    fn write_u64(&mut self, x: u64) {
        self.add(x);
    }
    fn write_usize(&mut self, x: usize) {
        self.add(x as u64);
    }
    fn write_i32(&mut self, x: i32) {
        self.add(x as u32 as u64);
    }
    fn write_i64(&mut self, x: i64) {
        self.add(x as u64);
    }
}

/// The content hash of anything hashable.
pub fn hash_of<T: Hash + ?Sized>(t: &T) -> u64 {
    let mut h = Fx(0);
    t.hash(&mut h);
    h.finish()
}

/// A `HashMap` with [`Fx`].
pub type FxMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<Fx>>;
/// A `HashSet` with [`Fx`].
pub type FxSet<K> = std::collections::HashSet<K, std::hash::BuildHasherDefault<Fx>>;
