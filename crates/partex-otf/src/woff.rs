//! WOFF 1.0 to sfnt, as FreeType's `woff_open_font` unpacks it (each
//! table inflated when its compressed length is below its original
//! length). WOFF2 is not unpacked here.

use alloc::vec::Vec;

use crate::face::{rd_u16, rd_u32};

/// Whether `data` is a WOFF (`wOFF`) file.
#[must_use]
pub fn is_woff(data: &[u8]) -> bool {
    data.get(0..4) == Some(b"wOFF")
}

/// Whether `data` is a WOFF2 (`wOF2`) file.
#[must_use]
pub fn is_woff2(data: &[u8]) -> bool {
    data.get(0..4) == Some(b"wOF2")
}

/// The sfnt a WOFF file holds, or `None` if it is not one or is damaged.
#[must_use]
pub fn decode(data: &[u8]) -> Option<Vec<u8>> {
    if !is_woff(data) {
        return None;
    }
    let flavor = rd_u32(data, 4)?;
    let n = rd_u16(data, 12)? as usize;
    let mut tables = Vec::with_capacity(n);
    for i in 0..n {
        let r = 44 + 20 * i;
        let tag = rd_u32(data, r)?;
        let off = rd_u32(data, r + 4)? as usize;
        let comp = rd_u32(data, r + 8)? as usize;
        let orig = rd_u32(data, r + 12)? as usize;
        let csum = rd_u32(data, r + 16)?;
        let raw = data.get(off..off + comp)?;
        let bytes = if comp < orig {
            let z = raw;
            if z.len() < 2 {
                return None;
            }
            let mut out = Vec::with_capacity(orig);
            crate::inflate::inflate_raw(&z[2..], &mut out);
            if out.len() != orig {
                return None;
            }
            out
        } else if comp == orig {
            raw.to_vec()
        } else {
            return None;
        };
        tables.push((tag, csum, bytes));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    let mut sr = 1u16;
    let mut es = 0u16;
    while usize::from(sr) * 2 <= n {
        sr *= 2;
        es += 1;
    }
    out.extend_from_slice(&(sr * 16).to_be_bytes());
    out.extend_from_slice(&es.to_be_bytes());
    out.extend_from_slice(&((n as u16) * 16 - sr * 16).to_be_bytes());
    let mut pos = 12 + 16 * n;
    let mut offs = Vec::with_capacity(n);
    for (_, _, b) in &tables {
        offs.push(pos);
        pos += (b.len() + 3) & !3;
    }
    for ((tag, csum, b), &o) in tables.iter().zip(&offs) {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&csum.to_be_bytes());
        out.extend_from_slice(&(o as u32).to_be_bytes());
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    }
    for (_, _, b) in &tables {
        out.extend_from_slice(b);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Some(out)
}
