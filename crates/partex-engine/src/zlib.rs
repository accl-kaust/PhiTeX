//! The zlib container, for PDF streams when no compressor is available:
//! stored (uncompressed) deflate blocks. Readers accept it; it is not
//! what zlib itself would write (a host supplies that).

use alloc::vec::Vec;

/// Adler-32 (RFC 1950).
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &c in chunk {
            a += u32::from(c);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// CRC-32 (ISO 3309, zlib's `crc32`).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// `data` as a zlib stream of stored blocks.
#[must_use]
pub fn stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 11);
    out.extend_from_slice(&[0x78, 0x01]);
    let mut chunks = data.chunks(65535).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        out.push(u8::from(chunks.peek().is_none()));
        let n = u16::try_from(c.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// The length of [`stored`]'s output for `n` bytes: its header, a
/// block's five bytes for each 65,535 bytes (one for none), the bytes,
/// and the checksum.
#[must_use]
pub fn stored_len(n: usize) -> usize {
    2 + 5 * n.div_ceil(65535).max(1) + n + 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn stored_empty() {
        assert_eq!(stored(b""), [0x78, 1, 1, 0, 0, 0xFF, 0xFF, 0, 0, 0, 1]);
    }

    #[test]
    fn stored_lengths() {
        for n in [0, 1, 65_534, 65_535, 65_536, 131_070, 200_000] {
            assert_eq!(
                stored(&alloc::vec![7u8; n]).len(),
                stored_len(n),
                "{n} bytes"
            );
        }
    }
}
