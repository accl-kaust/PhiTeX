//! Deterministic hashing: a fast hasher for internal tables (keys are mostly
//! already hashes) and a strong mixer for content ids.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// FxHash-style hasher. Deterministic (no random seed).
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const FX_K: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(FX_K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        let mut last = 0u64;
        for (i, b) in rest.iter().enumerate() {
            last |= u64::from(*b) << (8 * i);
        }
        self.add(last ^ bytes.len() as u64);
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = HashMap<K, V, FxBuild>;
pub type FxHashSet<K> = HashSet<K, FxBuild>;

const P0: u64 = 0xa076_1d64_78bd_642f;
const P1: u64 = 0xe703_7ed1_a0b4_28db;
const P2: u64 = 0x8ebc_6af0_9c88_c6e3;

/// 64x64 -> 128 multiply, folded (wyhash's "mum").
#[inline]
fn mum(a: u64, b: u64) -> u64 {
    let r = u128::from(a) * u128::from(b);
    #[allow(clippy::cast_possible_truncation)]
    let lo = r as u64;
    #[allow(clippy::cast_possible_truncation)]
    let hi = (r >> 64) as u64;
    lo ^ hi
}

/// Strong deterministic hash of a word sequence, for content ids.
#[must_use]
pub fn hash_words(seed: u64, words: &[u64]) -> u64 {
    let mut h = seed ^ P2;
    for &w in words {
        h = mum(h ^ P0, w ^ P1);
    }
    mum(h ^ words.len() as u64, P2)
}

/// Strong deterministic hash of bytes.
#[must_use]
pub fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed ^ P1;
    let (chunks, rest) = bytes.as_chunks::<8>();
    for c in chunks {
        h = mum(h ^ P0, u64::from_le_bytes(*c) ^ P1);
    }
    let mut last = 0u64;
    for (i, b) in rest.iter().enumerate() {
        last |= u64::from(*b) << (8 * i);
    }
    mum(h ^ last ^ P2, bytes.len() as u64 ^ P0)
}
