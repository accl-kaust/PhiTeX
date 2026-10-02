//! A stable 128-bit hash: the same value on every platform and run, for
//! keys that outlive a process (checkpoints, memo databases).
//!
//! Two 64-bit lanes with different seeds, each absorbing 8-byte words with
//! a multiply-rotate step, finished by a strong avalanche. It is not
//! cryptographic; collisions between the values an engine produces are
//! what it guards against.

use core::hash::{Hash, Hasher};

const K1: u64 = 0x9E37_79B9_7F4A_7C15;
const K2: u64 = 0xC2B2_AE3D_27D4_EB4F;

/// A [`Hasher`] with a stable, platform-independent 128-bit result
/// ([`StableHasher::finish128`]).
#[derive(Clone, Debug)]
pub struct StableHasher {
    a: u64,
    b: u64,
    /// Bytes waiting to fill a word, and how many.
    tail: u64,
    ntail: u32,
    len: u64,
}

impl Default for StableHasher {
    fn default() -> Self {
        Self {
            a: 0x243F_6A88_85A3_08D3,
            b: 0x1319_8A2E_0370_7344,
            tail: 0,
            ntail: 0,
            len: 0,
        }
    }
}

#[inline]
fn mix(x: u64) -> u64 {
    // splitmix64's finalizer
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl StableHasher {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn word(&mut self, w: u64) {
        self.a = (self.a ^ w).wrapping_mul(K1).rotate_left(29);
        self.b = (self.b.rotate_left(17) ^ w).wrapping_mul(K2);
    }

    /// The 128-bit result.
    #[must_use]
    pub fn finish128(&self) -> u128 {
        let mut s = self.clone();
        if s.ntail > 0 {
            let t = s.tail;
            s.word(t ^ 0xFF);
        }
        let a = mix(s.a ^ s.len);
        let b = mix(s.b ^ a ^ s.len.rotate_left(32));
        (u128::from(a) << 64) | u128::from(mix(a ^ b))
    }

    /// The 128-bit hash of `v`.
    #[must_use]
    pub fn of<T: Hash + ?Sized>(v: &T) -> u128 {
        let mut h = Self::new();
        v.hash(&mut h);
        h.finish128()
    }
}

impl Hasher for StableHasher {
    fn finish(&self) -> u64 {
        // (truncation intended: the low half of the 128-bit result)
        #[allow(clippy::cast_possible_truncation)]
        let low = self.finish128() as u64;
        low
    }

    fn write(&mut self, bytes: &[u8]) {
        self.len = self.len.wrapping_add(bytes.len() as u64);
        let mut rest = bytes;
        while self.ntail > 0 && !rest.is_empty() {
            self.tail |= u64::from(rest[0]) << (8 * self.ntail);
            self.ntail += 1;
            rest = &rest[1..];
            if self.ntail == 8 {
                let t = self.tail;
                self.word(t);
                self.tail = 0;
                self.ntail = 0;
            }
        }
        let (words, remainder) = rest.as_chunks::<8>();
        for c in words {
            self.word(u64::from_le_bytes(*c));
        }
        for &b in remainder {
            self.tail |= u64::from(b) << (8 * self.ntail);
            self.ntail += 1;
        }
    }

    // Integers are one word each, in little-endian order on every platform.
    fn write_u8(&mut self, i: u8) {
        self.write_u64(u64::from(i));
    }
    fn write_u16(&mut self, i: u16) {
        self.write_u64(u64::from(i));
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }
    fn write_u64(&mut self, i: u64) {
        self.len = self.len.wrapping_add(8);
        self.word(i);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    fn write_i8(&mut self, i: i8) {
        self.write_u64(i64::from(i).cast_unsigned());
    }
    fn write_i16(&mut self, i: i16) {
        self.write_u64(i64::from(i).cast_unsigned());
    }
    fn write_i32(&mut self, i: i32) {
        self.write_u64(i64::from(i).cast_unsigned());
    }
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i.cast_unsigned());
    }
    fn write_isize(&mut self, i: isize) {
        self.write_u64((i as i64).cast_unsigned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_values() {
        // Keys live on disk: these must never change.
        assert_eq!(StableHasher::of(&0u32), StableHasher::of(&0u64));
        assert_ne!(StableHasher::of(&[1i32, 2]), StableHasher::of(&[2i32, 1]));
        assert_ne!(
            StableHasher::of(b"ab".as_slice()),
            StableHasher::of(b"ba".as_slice())
        );
        let mut h = StableHasher::new();
        h.write(b"abc");
        h.write(b"defghijk");
        let mut g = StableHasher::new();
        g.write(b"abcdefghijk");
        assert_eq!(h.finish128(), g.finish128());
    }
}
