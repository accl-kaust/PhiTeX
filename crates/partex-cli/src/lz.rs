//! A small, fast LZ77 codec for the store's blobs (`store.rs`): the LZ4
//! scheme (a token with literal and match lengths, the literals, a 16-bit
//! offset), with lengths past the token's nibble as LEB128, so a long run
//! of zeros (much of a machine state) costs a few bytes. Safe Rust, no
//! dependency; what matters is speed, both ways, over ratio.
//!
//! Encoded: the decoded length (LEB128), then sequences. Each sequence is
//! a token (literals in its high nibble, match length minus `MIN` in its
//! low; 15 means more follows as LEB128), the literals, and then, unless
//! the input ends there, the match's offset back (2 bytes, little-endian,
//! at least 1).

// (lengths and offsets are cut to bytes on purpose, and bounded before)
#![allow(clippy::cast_possible_truncation)]

/// The shortest match.
const MIN: usize = 4;
/// The hash table's bits, at most (fewer for a short input).
const BITS: u32 = 14;
/// The furthest offset.
const WINDOW: usize = 65_535;

fn put_len(out: &mut Vec<u8>, mut n: usize) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

fn get_len(b: &[u8], at: &mut usize) -> Option<usize> {
    let mut n: usize = 0;
    let mut shift = 0;
    loop {
        let c = *b.get(*at)?;
        *at += 1;
        n |= usize::from(c & 0x7f).checked_shl(shift)?;
        if c < 0x80 {
            return Some(n);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn hash4(b: &[u8], i: usize, bits: u32) -> usize {
    let v = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    (v.wrapping_mul(0x9e37_79b1) >> (32 - bits)) as usize
}

fn sequence(out: &mut Vec<u8>, lit: &[u8], m: Option<(usize, usize)>) {
    let ml = m.map_or(0, |(_, l)| l - MIN);
    let token = (lit.len().min(15) << 4) | ml.min(15);
    out.push(token as u8);
    if lit.len() >= 15 {
        put_len(out, lit.len() - 15);
    }
    out.extend_from_slice(lit);
    if let Some((off, _)) = m {
        out.extend_from_slice(&(off as u16).to_le_bytes());
        if ml >= 15 {
            put_len(out, ml - 15);
        }
    }
}

/// `data`, encoded.
#[allow(clippy::many_single_char_names)] // (the usual names)
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    put_len(&mut out, data.len());
    let n = data.len();
    let bits = n.bit_width().clamp(6, BITS);
    let mut table = vec![u32::MAX; 1 << bits];
    let (mut i, mut lit) = (0, 0);
    let mut misses = 0usize;
    while i + MIN <= n {
        let h = hash4(data, i, bits);
        let cand = table[h] as usize;
        table[h] = i as u32;
        if cand != u32::MAX as usize
            && i - cand <= WINDOW
            && data[cand..cand + MIN] == data[i..i + MIN]
        {
            let mut l = MIN;
            while i + l < n && data[cand + l] == data[i + l] {
                l += 1;
            }
            sequence(&mut out, &data[lit..i], Some((i - cand, l)));
            // (the table learns a few positions of the match)
            let end = i + l;
            let mut j = i + 1;
            while j + MIN <= n && j < end.min(i + 8) {
                table[hash4(data, j, bits)] = j as u32;
                j += 1;
            }
            i = end;
            lit = i;
            misses = 0;
        } else {
            // (past many misses, skip ahead faster: incompressible data)
            misses += 1;
            i += 1 + (misses >> 6);
        }
    }
    sequence(&mut out, &data[lit..], None);
    out
}

/// What `enc` encodes, if it is well formed.
pub fn decode(enc: &[u8]) -> Option<Vec<u8>> {
    let mut at = 0;
    let n = get_len(enc, &mut at)?;
    // (grown past a bound on what the length could honestly be)
    let mut out: Vec<u8> = Vec::with_capacity(n.min(enc.len().saturating_mul(64)));
    while at < enc.len() {
        let token = enc[at];
        at += 1;
        let mut ll = usize::from(token >> 4);
        if ll == 15 {
            ll = ll.checked_add(get_len(enc, &mut at)?)?;
        }
        let lits = enc.get(at..at.checked_add(ll)?)?;
        if out.len() + ll > n {
            return None;
        }
        out.extend_from_slice(lits);
        at += ll;
        if at == enc.len() {
            break;
        }
        let off = usize::from(u16::from_le_bytes([*enc.get(at)?, *enc.get(at + 1)?]));
        at += 2;
        let mut ml = usize::from(token & 15);
        if ml == 15 {
            ml = ml.checked_add(get_len(enc, &mut at)?)?;
        }
        let ml = ml.checked_add(MIN)?;
        if off == 0 || off > out.len() || out.len() + ml > n {
            return None;
        }
        let start = out.len() - off;
        if off >= ml {
            out.extend_from_within(start..start + ml);
        } else if off == 1 {
            let b = out[start];
            out.resize(out.len() + ml, b);
        } else {
            // (an overlapping copy repeats the last `off` bytes)
            let mut left = ml;
            while left > 0 {
                let k = left.min(out.len() - start);
                out.extend_from_within(start..start + k);
                left -= k;
            }
        }
    }
    (out.len() == n).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    fn round(d: &[u8]) {
        let e = encode(d);
        assert_eq!(decode(&e).as_deref(), Some(d), "{} bytes", d.len());
    }

    #[test]
    fn round_trips() {
        round(b"");
        round(b"a");
        round(b"abcd");
        round(&vec![0; 1_000_000]);
        round(b"abcabcabcabcabcabcabcabcabcabcabcabcabcabcx");
        let mut x: u64 = 1;
        let noise: Vec<u8> = (0..300_000)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                if i % 7 < 3 { 0 } else { (x >> 56) as u8 }
            })
            .collect();
        round(&noise);
        let mut mixed = noise.clone();
        mixed.extend_from_slice(&vec![7; 100_000]);
        mixed.extend_from_slice(&noise[1000..90_000]);
        round(&mixed);
        assert!(encode(&vec![0; 1_000_000]).len() < 40);
    }

    #[test]
    fn refuses_what_is_not_an_encoding() {
        let e = encode(b"abcabcabcabcabcabcabcabc and more text here");
        for cut in 0..e.len() {
            let _ = decode(&e[..cut]);
        }
        for i in 0..e.len() {
            let mut f = e.clone();
            f[i] ^= 0x5a;
            let _ = decode(&f);
        }
        assert_eq!(decode(&[5, 0x50, b'a']), None);
    }
}
