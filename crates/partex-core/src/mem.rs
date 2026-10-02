//! Part 8: Packed data (§110–§114), as built by web2c. (Part 9's `mem`
//! is gone: lists are the engine's typed nodes.)
//!
//! [`MemoryWord`] reproduces web2c's little-endian "big TeX" `memoryword`
//! union (texmfmem.h) bit for bit, so writing one variant and reading another
//! behaves exactly like the reference binary:
//!
//! ```text
//! byte:          0     1     2     3     4     5     6     7
//! hh (halves):   ---------LH----------   ---------RH----------
//! hh.b1, hh.b0:  ----B1----  ----B0----
//! int / sc:                              ---------INT---------
//! qqqq:                                  B3    B2    B1    B0
//! gr:            -------------------double--------------------
//! ```

#[allow(unused_imports)]
pub use crate::web::{
    EMPTY_FLAG, MAX_HALFWORD, MAX_QUARTERWORD, MIN_HALFWORD, MIN_QUARTERWORD, NULL, SMALL_NODE_SIZE,
};

/// §115: a pointer into `mem` or `eqtb`, or a flag.
pub type Pointer = i32;

/// §113: one `memory_word`, stored in web2c's byte layout.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryWord(u64);

/// In bulk: the engine's tables are mostly memory words.
impl partex_engine::persist::Persist for MemoryWord {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        partex_engine::persist::Persist::save(&self.0, s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self(partex_engine::persist::Persist::load(l)?))
    }
    fn save_slice(v: &[Self], s: &mut partex_engine::persist::Saver) {
        let b = &mut s.enc.0;
        b.reserve(v.len() * 8);
        for x in v {
            b.extend_from_slice(&x.0.to_le_bytes());
        }
    }
    fn load_vec(n: usize, l: &mut partex_engine::persist::Loader) -> Option<alloc::vec::Vec<Self>> {
        Some(
            <u64 as partex_engine::persist::Persist>::load_vec(n, l)?
                .into_iter()
                .map(Self)
                .collect(),
        )
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
impl MemoryWord {
    /// The raw bits (for format files).
    #[inline]
    pub fn to_bits(self) -> u64 {
        self.0
    }
    #[inline]
    pub fn from_bits(b: u64) -> Self {
        Self(b)
    }
    #[inline]
    #[must_use]
    pub fn lh(self) -> i32 {
        self.0 as u32 as i32
    }
    #[inline]
    #[must_use]
    pub fn rh(self) -> i32 {
        (self.0 >> 32) as u32 as i32
    }
    #[inline]
    pub fn set_lh(&mut self, v: i32) {
        self.0 = (self.0 & !0xFFFF_FFFF) | u64::from(v as u32);
    }
    #[inline]
    pub fn set_rh(&mut self, v: i32) {
        self.0 = (self.0 & 0xFFFF_FFFF) | (u64::from(v as u32) << 32);
    }
    /// `hh.b0`: a C `short` in the upper half of `lh`.
    #[inline]
    #[must_use]
    pub fn b0(self) -> i32 {
        i32::from((self.0 >> 16) as u16 as i16)
    }
    /// `hh.b1`: a C `short` in the lower half of `lh`.
    #[inline]
    #[must_use]
    pub fn b1(self) -> i32 {
        i32::from(self.0 as u16 as i16)
    }
    #[inline]
    pub fn set_b0(&mut self, v: i32) {
        self.0 = (self.0 & !0xFFFF_0000) | (u64::from(v as u16) << 16);
    }
    #[inline]
    pub fn set_b1(&mut self, v: i32) {
        self.0 = (self.0 & !0xFFFF) | u64::from(v as u16);
    }
    /// All the bits (for hashing).
    #[inline]
    #[must_use]
    pub fn bits(self) -> u64 {
        self.0
    }
    /// `int` and `sc` share the bytes of `rh`.
    #[inline]
    #[must_use]
    pub fn int(self) -> i32 {
        self.rh()
    }
    #[inline]
    pub fn set_int(&mut self, v: i32) {
        self.set_rh(v);
    }
    /// `qqqq.b0` … `qqqq.b3`: unsigned bytes of `rh`, `b0` most significant.
    #[inline]
    #[must_use]
    pub fn qqqq(self, i: u32) -> i32 {
        i32::from((self.0 >> (56 - 8 * i)) as u8)
    }
    #[inline]
    pub fn set_qqqq(&mut self, i: u32, v: i32) {
        let shift = 56 - 8 * i;
        self.0 = (self.0 & !(0xFF << shift)) | (u64::from(v as u8) << shift);
    }
    /// `gr`: the whole word as a `double`.
    #[inline]
    #[must_use]
    pub fn gr(self) -> f64 {
        f64::from_bits(self.0)
    }
    #[inline]
    pub fn set_gr(&mut self, v: f64) {
        self.0 = v.to_bits();
    }
}

impl core::fmt::Debug for MemoryWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[lh={} rh={}]", self.lh(), self.rh())
    }
}

/// §124: size fields and links of empty variable-size nodes.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_layout_matches_web2c() {
        let mut w = MemoryWord::default();
        w.set_rh(0x0102_0304);
        assert_eq!(w.int(), 0x0102_0304);
        assert_eq!([w.qqqq(0), w.qqqq(1), w.qqqq(2), w.qqqq(3)], [1, 2, 3, 4]);
        w.set_lh(-2);
        assert_eq!((w.b0(), w.b1()), (-1, -2));
        w.set_b0(7);
        w.set_b1(300);
        assert_eq!(w.lh(), (7 << 16) | 0x12C);
        assert_eq!(w.rh(), 0x0102_0304);
        w.set_qqqq(0, 0xAB);
        assert_eq!(w.int(), 0xAB02_0304_u32.cast_signed());
        w.set_gr(1.5);
        assert!((w.gr() - 1.5).abs() < f64::EPSILON);
        // 1.5 = 0x3FF8_0000_0000_0000: its high half is `rh`.
        assert_eq!(w.rh(), 0x3FF8_0000);
        assert_eq!(w.lh(), 0);
    }
}
