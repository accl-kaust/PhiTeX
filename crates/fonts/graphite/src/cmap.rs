// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/CmapCache.cpp, src/inc/CmapCache.h),
// LGPL-2.1-or-later.

//! The character map, cached as `gr_face_cacheCmap` caches it
//! (`CachedCmap`): the Unicode BMP subtable (3,1 or 0,3..0,0, format 4)
//! over the full-repertoire one (3,10 or 0,4, format 12), in 256-entry
//! blocks.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::ttf;

pub(crate) struct Cmap {
    bmp_only: bool,
    blocks: Vec<Option<Box<[u16; 256]>>>,
}

fn bmp_subtable(cmap: &[u8]) -> Option<usize> {
    if cmap.is_empty() {
        return None;
    }
    for (p, e) in [(3, 1), (0, 3), (0, 2), (0, 1), (0, 0)] {
        let st = ttf::find_cmap_subtable(cmap, p, e);
        if ttf::check_cmap_subtable4(cmap, st) {
            return st;
        }
    }
    None
}

fn smp_subtable(cmap: &[u8]) -> Option<usize> {
    if cmap.is_empty() {
        return None;
    }
    for (p, e) in [(3, 10), (0, 4)] {
        let st = ttf::find_cmap_subtable(cmap, p, e);
        if ttf::check_cmap_subtable12(cmap, st) {
            return st;
        }
    }
    None
}

/// `cache_subtable`: every code point the subtable lists below `limit`.
fn cache_subtable(
    blocks: &mut [Option<Box<[u16; 256]>>],
    cmap: &[u8],
    st: usize,
    limit: u32,
    next: fn(&[u8], usize, u32, &mut i32) -> u32,
    lookup: fn(&[u8], usize, u32, i32) -> u16,
) {
    let mut key = 0i32;
    let mut cp = next(cmap, st, 0, &mut key);
    let mut prev = 0u32;
    while cp < limit {
        let block = (cp >> 8) as usize;
        let b = blocks[block].get_or_insert_with(|| Box::new([0; 256]));
        b[(cp & 0xFF) as usize] = lookup(cmap, st, cp, key);
        if cp <= prev {
            cp = prev + 1;
        }
        prev = cp;
        cp = next(cmap, st, cp, &mut key);
    }
}

impl Cmap {
    /// `CachedCmap(face)`; `None` without a `cmap` table (`operator
    /// bool`).
    pub(crate) fn new(cmap: Option<&[u8]>) -> Option<Cmap> {
        let cmap = cmap?;
        let bmp = bmp_subtable(cmap);
        let smp = smp_subtable(cmap);
        let bmp_only = smp.is_none();
        let mut blocks: Vec<Option<Box<[u16; 256]>>> = Vec::new();
        blocks.resize_with(if bmp_only { 0x100 } else { 0x1100 }, || None);
        if let Some(st) = smp {
            cache_subtable(
                &mut blocks,
                cmap,
                st,
                0x10FFFF,
                ttf::cmap_subtable12_next,
                ttf::cmap_subtable12_lookup,
            );
        }
        if let Some(st) = bmp {
            cache_subtable(
                &mut blocks,
                cmap,
                st,
                0xFFFF,
                ttf::cmap_subtable4_next,
                ttf::cmap_subtable4_lookup,
            );
        }
        Some(Cmap { bmp_only, blocks })
    }

    /// `operator[]`.
    pub(crate) fn get(&self, usv: u32) -> u16 {
        if (self.bmp_only && usv > 0xFFFF) || usv > 0x10FFFF {
            return 0;
        }
        let block = (0xFFFF & (usv >> 8)) as usize;
        match &self.blocks[block] {
            Some(b) => b[(usv & 0xFF) as usize],
            None => 0,
        }
    }
}
