//! Stable content hashing (`DESIGN.md` §7.1, "Stable hashing").
//!
//! A cell's version is the 128-bit hash of its value, independent of
//! platform, pointer width and run: every integer is fed little-endian and
//! `usize` as 64 bits. Two writes of the same value give the same version,
//! which is what makes early cutoff automatic.

use core::hash::{Hash, Hasher};

/// A content version: the stable hash of a value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Version(pub u128);

/// The stable hash of `v` (of `Option<&V>` for a cell that may be absent).
pub fn version_of<T: Hash + ?Sized>(v: &T) -> Version {
    let mut h = StableHasher::new();
    v.hash(&mut h);
    Version(h.finish128())
}

const K1: u64 = 0x9e37_79b9_7f4a_7c15;
const K2: u64 = 0xc2b2_ae3d_27d4_eb4f;

/// A two-lane multiply-rotate hasher with a 128-bit result. Not
/// cryptographic; stable across platforms and runs (never `SipHasher`).
#[derive(Clone, Debug)]
pub struct StableHasher {
    a: u64,
    b: u64,
    len: u64,
    buf: [u8; 8],
    nbuf: usize,
}

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    #[must_use]
    pub fn new() -> Self {
        Self {
            a: 0x243f_6a88_85a3_08d3,
            b: 0x1319_8a2e_0370_7344,
            len: 0,
            buf: [0; 8],
            nbuf: 0,
        }
    }

    #[inline]
    fn mix(&mut self, w: u64) {
        self.a = (self.a ^ w).wrapping_mul(K1).rotate_left(29);
        self.b = (self.b.rotate_left(17) ^ w).wrapping_mul(K2) ^ self.a;
    }

    /// The 128-bit digest of everything written so far.
    #[must_use]
    pub fn finish128(&self) -> u128 {
        let mut s = self.clone();
        if s.nbuf > 0 {
            let mut w = [0u8; 8];
            w[..s.nbuf].copy_from_slice(&s.buf[..s.nbuf]);
            s.mix(u64::from_le_bytes(w));
        }
        s.mix(s.len);
        let a = fmix(s.a ^ s.b.rotate_left(32));
        let b = fmix(s.b ^ a);
        (u128::from(a) << 64) | u128::from(b)
    }
}

#[inline]
fn fmix(mut k: u64) -> u64 {
    k ^= k >> 33;
    k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
    k ^= k >> 33;
    k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    k ^ (k >> 33)
}

impl Hasher for StableHasher {
    fn write(&mut self, mut bytes: &[u8]) {
        self.len = self.len.wrapping_add(bytes.len() as u64);
        if self.nbuf > 0 {
            let take = (8 - self.nbuf).min(bytes.len());
            self.buf[self.nbuf..self.nbuf + take].copy_from_slice(&bytes[..take]);
            self.nbuf += take;
            bytes = &bytes[take..];
            if self.nbuf < 8 {
                return;
            }
            let w = u64::from_le_bytes(self.buf);
            self.mix(w);
            self.nbuf = 0;
        }
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.mix(u64::from_le_bytes(*c));
        }
        self.buf[..rest.len()].copy_from_slice(rest);
        self.nbuf = rest.len();
    }

    fn write_u8(&mut self, i: u8) {
        self.write(&[i]);
    }
    fn write_u16(&mut self, i: u16) {
        self.write(&i.to_le_bytes());
    }
    fn write_u32(&mut self, i: u32) {
        self.write(&i.to_le_bytes());
    }
    fn write_u64(&mut self, i: u64) {
        self.write(&i.to_le_bytes());
    }
    fn write_u128(&mut self, i: u128) {
        self.write(&i.to_le_bytes());
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    fn write_i8(&mut self, i: i8) {
        self.write(&i.to_le_bytes());
    }
    fn write_i16(&mut self, i: i16) {
        self.write(&i.to_le_bytes());
    }
    fn write_i32(&mut self, i: i32) {
        self.write(&i.to_le_bytes());
    }
    fn write_i64(&mut self, i: i64) {
        self.write(&i.to_le_bytes());
    }
    fn write_i128(&mut self, i: i128) {
        self.write(&i.to_le_bytes());
    }
    fn write_isize(&mut self, i: isize) {
        self.write_i64(i as i64);
    }

    fn finish(&self) -> u64 {
        // Truncation intended: the low half of the 128-bit digest.
        #[allow(clippy::cast_possible_truncation)]
        let low = self.finish128() as u64;
        low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_split_invariant() {
        let v = version_of(&(1u32, "abc", Some(7i64)));
        assert_eq!(v, version_of(&(1u32, "abc", Some(7i64))));
        let mut h1 = StableHasher::new();
        h1.write(b"hello world, this is longer than eight");
        let mut h2 = StableHasher::new();
        h2.write(b"hello w");
        h2.write(b"orld, this is longer");
        h2.write(b" than eight");
        assert_eq!(h1.finish128(), h2.finish128());
        assert_ne!(version_of(&1u64), version_of(&2u64));
        assert_ne!(version_of(&None::<u8>), version_of(&Some(0u8)));
        // `Option<&V>` and `Option<V>` hash alike: guards rely on it.
        assert_eq!(version_of(&Some(&5u32)), version_of(&Some(5u32)));
    }
}
