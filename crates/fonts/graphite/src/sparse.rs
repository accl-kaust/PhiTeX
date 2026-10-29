// Copyright 2011, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Sparse.cpp, src/inc/Sparse.h),
// LGPL-2.1-or-later.

//! `sparse`: a glyph's attributes, a packed map from `u16` keys to
//! nonzero `u16` values (0 where a key is missing).

use alloc::vec::Vec;

/// `SIZEOF_CHUNK`: a chunk's mask bits (`unsigned long` less the offset).
const CHUNK: u32 = 48;

#[derive(Clone, Debug, Default)]
pub(crate) struct Sparse {
    /// (mask, offset into `values`) per chunk.
    chunks: Vec<(u64, u16)>,
    values: Vec<u16>,
    /// Whether it was made (`operator bool`): false where the keys were
    /// not increasing.
    valid: bool,
}

impl Sparse {
    /// `sparse(first, last)` over (key, value) pairs.
    pub(crate) fn new(pairs: impl Iterator<Item = (u16, u16)> + Clone) -> Sparse {
        let mut nchunks: u32 = 0;
        let mut lastkey: i64 = -1;
        let mut count = 0usize;
        for (k, v) in pairs.clone() {
            if v == 0 {
                continue;
            }
            count += 1;
            if i64::from(k) <= lastkey {
                return Sparse {
                    chunks: Vec::new(),
                    values: Vec::new(),
                    valid: false,
                };
            }
            lastkey = i64::from(k);
            let c = u32::from(k) / CHUNK;
            if c >= nchunks {
                nchunks = c + 1;
            }
        }
        if nchunks == 0 {
            return Sparse {
                chunks: Vec::new(),
                values: Vec::new(),
                valid: true,
            };
        }
        let mut chunks = alloc::vec![(0u64, 0u16); nchunks as usize];
        let mut values = Vec::with_capacity(count);
        let mut ci: usize = 0;
        for (k, v) in pairs {
            if v == 0 {
                continue;
            }
            let c = (u32::from(k) / CHUNK) as usize;
            if c != ci {
                ci = c;
                chunks[ci].1 = values.len() as u16;
            }
            chunks[ci].0 |= 1u64 << (CHUNK - 1 - u32::from(k) % CHUNK);
            values.push(v);
        }
        Sparse {
            chunks,
            values,
            valid: true,
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.valid
    }

    /// `capacity()`: the values stored.
    pub(crate) fn capacity(&self) -> usize {
        self.values.len()
    }

    /// `operator[]`.
    pub(crate) fn get(&self, k: u16) -> u16 {
        let c = (u32::from(k) / CHUNK) as usize;
        let Some(&(mask, offset)) = self.chunks.get(c) else {
            return 0;
        };
        let m = mask >> (CHUNK - 1 - u32::from(k) % CHUNK);
        if m & 1 == 0 {
            return 0;
        }
        self.values[usize::from(offset) + (m >> 1).count_ones() as usize]
    }
}
