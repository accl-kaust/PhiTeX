//! MD5 (RFC 1321), for pdfTeX's `\pdfmdfivesum` and the PDF file
//! identifier. Not for security: TeX only uses it to fingerprint text.

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// `floor(abs(sin(i + 1)) * 2^32)`.
const K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

fn block(h: &mut [u32; 4], chunk: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (w, b) in m.iter_mut().zip(chunk.as_chunks::<4>().0) {
        *w = u32::from_le_bytes(*b);
    }
    let [mut a, mut b, mut c, mut d] = *h;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let t = d;
        d = c;
        c = b;
        b = b.wrapping_add(
            a.wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(m[g])
                .rotate_left(S[i]),
        );
        a = t;
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d]) {
        *x = x.wrapping_add(y);
    }
}

/// The MD5 digest of `data`.
#[must_use]
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut h: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let (chunks, rest) = data.as_chunks::<64>();
    for c in chunks {
        block(&mut h, c);
    }
    // the padding: 0x80, zeros, the length in bits
    let mut last = [0u8; 128];
    last[..rest.len()].copy_from_slice(rest);
    last[rest.len()] = 0x80;
    let n = if rest.len() < 56 { 64 } else { 128 };
    let bits = (data.len() as u64).wrapping_mul(8);
    last[n - 8..n].copy_from_slice(&bits.to_le_bytes());
    for c in last[..n].as_chunks::<64>().0 {
        block(&mut h, c);
    }
    let mut out = [0u8; 16];
    for (o, w) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = w.to_le_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::md5;

    fn hex(d: [u8; 16]) -> alloc::string::String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        d.iter()
            .flat_map(|&b| [HEX[usize::from(b >> 4)], HEX[usize::from(b & 15)]])
            .map(char::from)
            .collect()
    }

    #[test]
    fn rfc1321_vectors() {
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        assert_eq!(hex(md5(&[b'a'; 64])), "014842d480b571495a4a0363793f7367");
    }
}
