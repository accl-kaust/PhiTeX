// Copyright 2015, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Decompressor.cpp,
// src/inc/Compression.h), LGPL-2.1-or-later.

//! The LZ4 block decoder graphite2 uses for compressed `Silf` and `Glat`
//! tables, with its checks. Its word-at-a-time copies (which may write
//! past a copy's end, later overwritten) give the bytes a plain forward
//! copy gives.

use alloc::vec;
use alloc::vec::Vec;

const MINMATCH: usize = 4;
const LASTLITERALS: usize = 5;
const MINCODA: usize = LASTLITERALS + 1;
const MINSRCSIZE: usize = 13;

fn align(p: usize) -> usize {
    (p + 7) & !7
}

fn read_literal(s: &[u8], i: &mut usize, mut l: u32) -> u32 {
    if l == 15 && *i != s.len() {
        loop {
            let b = s[*i];
            *i += 1;
            l = l.wrapping_add(u32::from(b));
            if !(b == 0xff && *i != s.len()) {
                break;
            }
        }
    }
    l
}

/// `lz4::decompress(in, in_size, out, out_size)`: the decoded bytes (as
/// many as were written, which the caller checks against `out_size`), or
/// `None` where the C++ returns -1.
pub(crate) fn decompress(src: &[u8], out_size: usize) -> Option<Vec<u8>> {
    let in_size = src.len();
    if out_size <= in_size || in_size < MINSRCSIZE {
        return None;
    }
    let end = in_size;
    let mut out = vec![0u8; out_size];
    let mut room = out_size;
    let mut dst = 0usize;
    let mut s = 0usize;
    let mut literal;
    let mut literal_len;
    loop {
        // read_sequence
        let token = src[s];
        s += 1;
        literal_len = read_literal(src, &mut s, u32::from(token >> 4)) as usize;
        literal = s;
        s = s.wrapping_add(literal_len);
        if s > end - 2 || s < literal {
            break;
        }
        let match_dist = usize::from(src[s]) | usize::from(src[s + 1]) << 8;
        s += 2;
        let match_len = read_literal(src, &mut s, u32::from(token & 0xf)) as usize + MINMATCH;
        if s > end - MINCODA {
            break;
        }
        if literal_len != 0 {
            if align(literal_len) > room {
                return None;
            }
            out[dst..dst + literal_len].copy_from_slice(&src[literal..literal + literal_len]);
            dst += literal_len;
            room -= literal_len;
        }
        if match_dist > dst
            || match_len > (room.wrapping_sub(LASTLITERALS) as u32) as usize
            || room < LASTLITERALS
            || match_dist == 0
        {
            return None;
        }
        let pcpy = dst - match_dist;
        for k in 0..match_len {
            out[dst + k] = out[pcpy + k];
        }
        dst += match_len;
        room -= match_len;
    }
    if literal > end.wrapping_sub(literal_len) || literal_len > room {
        return None;
    }
    out[dst..dst + literal_len].copy_from_slice(&src[literal..literal + literal_len]);
    dst += literal_len;
    out.truncate(dst);
    Some(out)
}
