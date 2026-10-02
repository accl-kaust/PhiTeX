//! Versions: stable 128-bit content hashes (`DESIGN.md` §7.17.1).
//!
//! A version is computed from a value's content when the value is made,
//! independent of platform, pointer width and run: integers are fed
//! little-endian and `usize` as 64 bits. Equal content gives equal
//! versions, which is what early cutoff compares.

use core::fmt;
use core::hash::{Hash, Hasher};

/// A content version.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Version(pub u128);

impl Version {
    /// The version of an absent slot, or of a stream that does not exist.
    pub const ABSENT: Version = Version(0x6162_7365_6e74_0000_0000_0000_0000_0001);

    /// The stable hash of `v`.
    pub fn of<T: Hash + ?Sized>(v: &T) -> Version {
        let mut h = Stable::new();
        v.hash(&mut h);
        Version(h.finish128())
    }

    /// A version made of a tag and parts' versions, without hashing
    /// their content again (a Merkle node).
    #[must_use]
    pub fn node(tag: u64, parts: &[Version]) -> Version {
        let mut h = Stable::new();
        h.word(tag);
        for p in parts {
            h.word128(p.0);
        }
        Version(h.finish128())
    }

    /// The version of an optional value: [`Version::ABSENT`] for `None`.
    #[must_use]
    pub fn opt(v: Option<Version>) -> Version {
        v.unwrap_or(Version::ABSENT)
    }

    /// The low 64 bits, as a table hash.
    #[must_use]
    pub fn low(self) -> u64 {
        // Truncation intended.
        #[allow(clippy::cast_possible_truncation)]
        let l = self.0 as u64;
        l
    }
}

impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:032x}", self.0)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:032x}", self.0)
    }
}

const M1: u64 = 0xa076_1d64_78bd_642f;
const M2: u64 = 0xe703_7ed1_a0b4_28db;
const M3: u64 = 0x8ebc_6af0_9c88_c6e3;

/// A two-lane multiply-xorshift hasher with a 128-bit digest. Not
/// cryptographic; stable across platforms and runs.
#[derive(Clone, Debug)]
pub struct Stable {
    a: u64,
    b: u64,
    n: u64,
    buf: u64,
    nbuf: u32,
}

impl Default for Stable {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn fold(x: u64, y: u64) -> u64 {
    let p = u128::from(x) * u128::from(y);
    // Truncation intended: the two halves of the product are folded.
    #[allow(clippy::cast_possible_truncation)]
    let r = (p as u64) ^ ((p >> 64) as u64);
    r
}

impl Stable {
    #[must_use]
    pub fn new() -> Self {
        Self {
            a: 0x5851_f42d_4c95_7f2d,
            b: 0x1405_7b7e_f767_814f,
            n: 0,
            buf: 0,
            nbuf: 0,
        }
    }

    /// Feed one 64-bit word.
    #[inline]
    pub fn word(&mut self, w: u64) {
        self.a = fold(self.a ^ w, M1);
        self.b = fold(self.b.rotate_left(23) ^ w, M2).wrapping_add(self.a);
        self.n = self.n.wrapping_add(8);
    }

    /// Feed one 128-bit word.
    #[inline]
    pub fn word128(&mut self, w: u128) {
        // Truncation intended: the two halves.
        #[allow(clippy::cast_possible_truncation)]
        let (lo, hi) = (w as u64, (w >> 64) as u64);
        self.word(lo);
        self.word(hi);
    }

    /// The digest of everything fed so far.
    #[must_use]
    pub fn finish128(&self) -> u128 {
        let mut s = self.clone();
        if s.nbuf > 0 {
            let b = s.buf;
            s.word(b ^ u64::from(s.nbuf) << 56);
        }
        let a = fold(s.a ^ s.n, M3);
        let b = fold(s.b ^ a.rotate_left(31), M1) ^ fold(a, M2);
        (u128::from(a) << 64) | u128::from(b)
    }
}

impl Hasher for Stable {
    fn write(&mut self, bytes: &[u8]) {
        for &x in bytes {
            self.buf |= u64::from(x) << (8 * self.nbuf);
            self.nbuf += 1;
            if self.nbuf == 8 {
                let w = self.buf;
                self.buf = 0;
                self.nbuf = 0;
                self.word(w);
            }
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.word(u64::from(i) | 0x100);
    }
    fn write_u16(&mut self, i: u16) {
        self.word(u64::from(i) | 0x2_0000);
    }
    fn write_u32(&mut self, i: u32) {
        self.word(u64::from(i) | 0x4_0000_0000);
    }
    fn write_u64(&mut self, i: u64) {
        self.word(i);
    }
    fn write_u128(&mut self, i: u128) {
        self.word128(i);
    }
    fn write_usize(&mut self, i: usize) {
        self.word(i as u64);
    }
    fn write_i8(&mut self, i: i8) {
        self.write_u8(i.cast_unsigned());
    }
    fn write_i16(&mut self, i: i16) {
        self.write_u16(i.cast_unsigned());
    }
    fn write_i32(&mut self, i: i32) {
        self.write_u32(i.cast_unsigned());
    }
    fn write_i64(&mut self, i: i64) {
        self.word(i.cast_unsigned());
    }
    fn write_i128(&mut self, i: i128) {
        self.word128(i.cast_unsigned());
    }
    fn write_isize(&mut self, i: isize) {
        self.word((i as i64).cast_unsigned());
    }
    fn finish(&self) -> u64 {
        // Truncation intended: the low half of the digest.
        #[allow(clippy::cast_possible_truncation)]
        let low = self.finish128() as u64;
        low
    }
}

/// A 64-bit table hash of `v` (the low half of its version).
pub fn hash64<T: Hash + ?Sized>(v: &T) -> u64 {
    Version::of(v).low()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_split_invariant() {
        assert_eq!(Version::of(&(1u32, "abc")), Version::of(&(1u32, "abc")));
        let mut h1 = Stable::new();
        h1.write(b"hello world, this is longer than eight");
        let mut h2 = Stable::new();
        h2.write(b"hello w");
        h2.write(b"orld, this is longer");
        h2.write(b" than eight");
        assert_eq!(h1.finish128(), h2.finish128());
        assert_ne!(Version::of(&1u64), Version::of(&2u64));
        assert_ne!(Version::of("ab"), Version::of("ab\0"));
        assert_ne!(Version::of(&None::<u8>), Version::of(&Some(0u8)));
    }
}
