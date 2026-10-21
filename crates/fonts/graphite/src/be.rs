// Copyright 2011, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/inc/Endian.h), LGPL-2.1-or-later.

//! Big-endian reads at byte offsets (`be::peek`/`be::read`). A read past
//! the end gives 0: graphite2 checks its bounds before it reads, so a
//! read past the end only happens where the C++ reads memory it does not
//! own.

#[inline]
pub(crate) fn u8_at(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

#[inline]
pub(crate) fn u16_at(d: &[u8], o: usize) -> u16 {
    match d.get(o..o.wrapping_add(2)) {
        Some(b) => u16::from_be_bytes([b[0], b[1]]),
        None => 0,
    }
}

#[inline]
pub(crate) fn i16_at(d: &[u8], o: usize) -> i16 {
    u16_at(d, o) as i16
}

#[inline]
pub(crate) fn u32_at(d: &[u8], o: usize) -> u32 {
    match d.get(o..o.wrapping_add(4)) {
        Some(b) => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        None => 0,
    }
}
