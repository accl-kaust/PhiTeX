// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/NameTable.cpp, src/inc/NameTable.h),
// LGPL-2.1-or-later.

//! The `name` table as graphite2 reads it for feature labels: the
//! Windows Unicode BMP records (platform 3, encoding 1), with its quirks
//! (only the run of such records that follows the first one, and record
//! 0 never chosen as a best match).

use alloc::string::String;
use alloc::vec::Vec;

use crate::be::u16_at;

pub(crate) struct NameTable {
    data: Vec<u8>,
    platform_offset: u16,
    platform_last: u16,
    /// Offset of the string storage, and its length (`uint16`).
    name_data: usize,
    name_data_len: u16,
}

impl NameTable {
    /// `NameTable(data, length)` (platform 3, encoding 1); `None` where
    /// `m_table` is null.
    pub(crate) fn new(d: &[u8]) -> Option<NameTable> {
        let length = d.len();
        let count = usize::from(u16_at(d, 2));
        if !(length > 18 && length > 18usize.wrapping_add(12usize.wrapping_mul(count.wrapping_sub(1)))) {
            return None;
        }
        let offset = usize::from(u16_at(d, 4));
        if offset >= length {
            return None;
        }
        let mut t = NameTable {
            data: d.to_vec(),
            platform_offset: 0,
            platform_last: 0,
            name_data: offset,
            name_data_len: 0,
        };
        t.set_platform_encoding(3, 1);
        t.name_data_len = (length - offset) as u16;
        Some(t)
    }

    fn rec(&self, i: u16, f: usize) -> u16 {
        u16_at(&self.data, 6 + 12 * usize::from(i) + f)
    }

    fn set_platform_encoding(&mut self, platform: u16, encoding: u16) {
        let count = u16_at(&self.data, 2);
        let mut i: u16 = 0;
        while i < count {
            if self.rec(i, 0) == platform && self.rec(i, 2) == encoding {
                self.platform_offset = i;
                break;
            }
            i += 1;
        }
        loop {
            i = i.wrapping_add(1);
            if !(i < count && self.rec(i, 0) == platform && self.rec(i, 2) == encoding) {
                break;
            }
            self.platform_last = i;
        }
    }

    /// `getName(languageId, nameId, gr_utf8, length)`: the string and the
    /// language it was found in.
    pub(crate) fn get_name(&self, language: u16, name_id: u16) -> Option<(String, u16)> {
        let (mut any, mut en_us, mut best) = (0u16, 0u16, 0u16);
        let mut i = self.platform_offset;
        while i <= self.platform_last {
            if self.rec(i, 6) == name_id {
                let lang = self.rec(i, 4);
                if lang == language {
                    best = i;
                    break;
                } else if lang & 0xFF == language & 0xFF {
                    best = i;
                } else if lang == 0x409 {
                    en_us = i;
                } else {
                    any = i;
                }
            }
            i = i.wrapping_add(1);
            if i == 0 {
                break;
            }
        }
        if best == 0 {
            if en_us != 0 {
                best = en_us;
            } else {
                best = any;
                if any == 0 {
                    return None;
                }
            }
        }
        let lang = self.rec(best, 4);
        let len = usize::from(self.rec(best, 8));
        let off = usize::from(self.rec(best, 10));
        if off + len > usize::from(self.name_data_len) {
            return None;
        }
        let units: Vec<u16> = (0..len >> 1).map(|k| u16_at(&self.data, self.name_data + off + 2 * k)).collect();
        // (utf16::validate: the last unit is not a lead surrogate)
        if units.last().is_some_and(|&u| (0xD800..=0xDBFF).contains(&u)) {
            return None;
        }
        Some((String::from_utf16_lossy(&units), lang))
    }
}
