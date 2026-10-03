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
    let (lo, hi) = mul_wide(x, y);
    lo ^ hi
}

/// The 128-bit product of `x` and `y`: its low and high halves.
#[inline]
fn mul_wide(x: u64, y: u64) -> (u64, u64) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let p = u128::from(x) * u128::from(y);
        // Truncation intended: the halves of the product.
        #[allow(clippy::cast_possible_truncation)]
        (p as u64, (p >> 64) as u64)
    }
    #[cfg(target_arch = "wasm32")]
    {
        mul_wide_limbs(x, y)
    }
}

/// `a · b` modulo 2⁶¹ − 1, for `a` and `b` below it: the product of the
/// polynomial hashes (a token list's, [`crate::pvec`]'s lanes), by
/// [`mul_wide`], so wasm32 makes it from 32-bit halves (it called the
/// compiler's `__multi3` for each token of every token list made).
#[inline]
#[must_use]
pub fn mulmod61(a: u64, b: u64) -> u64 {
    reduce61(mul_wide(a, b))
}

/// A product below 2¹²², as [`mul_wide`]'s halves, modulo 2⁶¹ − 1: its
/// low 61 bits plus the bits from 61 up (2⁶¹ is 1 there).
#[inline]
fn reduce61((lo, hi): (u64, u64)) -> u64 {
    const P: u64 = (1 << 61) - 1;
    let r = (lo & P) + ((hi << 3) | (lo >> 61));
    if r >= P { r - P } else { r }
}

/// [`mul_wide`] from the 32-bit halves, with 64-bit multiplies: wasm has
/// no 64×64→128 multiply, and the compiler's 128-bit routine (`__multi3`)
/// was 4% of a keystroke in the browser.
#[inline]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn mul_wide_limbs(x: u64, y: u64) -> (u64, u64) {
    const LOW: u64 = 0xffff_ffff;
    let (x0, x1) = (x & LOW, x >> 32);
    let (y0, y1) = (y & LOW, y >> 32);
    let (p00, p01, p10, p11) = (x0 * y0, x0 * y1, x1 * y0, x1 * y1);
    // (the middle column, under 3 × 2^32: no overflow)
    let mid = (p00 >> 32) + (p01 & LOW) + (p10 & LOW);
    let lo = (p00 & LOW) | (mid << 32);
    let hi = p11 + (p01 >> 32) + (p10 >> 32) + (mid >> 32);
    (lo, hi)
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

    /// Feed one byte: eight make a word, the first its lowest.
    #[inline]
    fn byte(&mut self, x: u8) {
        self.buf |= u64::from(x) << (8 * self.nbuf);
        self.nbuf += 1;
        if self.nbuf == 8 {
            let w = self.buf;
            self.buf = 0;
            self.nbuf = 0;
            self.word(w);
        }
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
        // (a byte at a time until the buffer is empty, then whole words,
        // eight bytes at a time: the words a byte at a time makes)
        let mut rest = bytes;
        while self.nbuf != 0 {
            let Some((&x, r)) = rest.split_first() else {
                return;
            };
            rest = r;
            self.byte(x);
        }
        let (words, tail) = rest.as_chunks::<8>();
        for w in words {
            self.word(u64::from_le_bytes(*w));
        }
        for &x in tail {
            self.byte(x);
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
    use core::hash::Hasher;

    /// Bytes fed whole, in pieces, or one at a time make the same digest.
    #[test]
    fn bytes_hash_alike_however_fed() {
        let data: alloc::vec::Vec<u8> = (0..200u32)
            .map(|i| u8::try_from(i.wrapping_mul(37) % 251).unwrap_or(0))
            .collect();
        for n in [0, 1, 7, 8, 9, 15, 16, 17, 63, 64, 65, 200] {
            let d = &data[..n];
            let mut one = super::Stable::new();
            for &x in d {
                one.byte(x);
            }
            let want = one.finish128();
            for cut in 0..=n.min(20) {
                let mut s = super::Stable::new();
                s.write(&d[..cut]);
                s.write(&d[cut..]);
                assert_eq!(s.finish128(), want, "{n} bytes cut at {cut}");
            }
        }
    }

    /// The product from 32-bit halves is the 128-bit product (wasm's
    /// path, checked natively).
    #[test]
    fn mul_wide_limbs_is_the_product() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let edges = [
            0,
            1,
            u64::MAX,
            u64::MAX - 1,
            1 << 32,
            (1 << 32) - 1,
            1 << 63,
        ];
        let check = |a: u64, b: u64| {
            let p = u128::from(a) * u128::from(b);
            #[allow(clippy::cast_possible_truncation)]
            let want = (p as u64, (p >> 64) as u64);
            assert_eq!(super::mul_wide_limbs(a, b), want, "{a:#x} * {b:#x}");
        };
        for &a in &edges {
            for &b in &edges {
                check(a, b);
            }
        }
        for _ in 0..100_000 {
            // (xorshift: inputs over the whole range)
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let y = x.rotate_left(29) ^ 0xd6e8_feb8_6659_fd93;
            check(x, y);
        }
    }

    /// [`mulmod61`] is the residue of the 128-bit product, by either
    /// product (wasm's from 32-bit halves, checked natively).
    #[test]
    fn mulmod61_is_the_residue() {
        const P: u64 = (1 << 61) - 1;
        let want = |a: u64, b: u64| {
            let x = u128::from(a) * u128::from(b);
            #[allow(clippy::cast_possible_truncation)]
            let r = ((x as u64) & P) + (x >> 61) as u64;
            if r >= P { r - P } else { r }
        };
        let check = |a: u64, b: u64| {
            assert_eq!(super::mulmod61(a, b), want(a, b), "{a:#x} * {b:#x}");
            assert_eq!(
                super::reduce61(super::mul_wide_limbs(a, b)),
                want(a, b),
                "{a:#x} * {b:#x} by halves"
            );
        };
        let edges = [0, 1, 2, P - 1, P - 2, 1 << 60, (1 << 32) - 1, 1 << 32];
        for &a in &edges {
            for &b in &edges {
                check(a, b);
            }
        }
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..100_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            check(x % P, x.rotate_left(23) % P);
        }
    }

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
