//! A stable 64-bit hash (FNV-1a), so that hashes in a trace mean the same
//! across builds and machines.

/// FNV-1a over `bytes`.
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    fnv1a_with(0xcbf2_9ce4_8422_2325, bytes)
}

/// FNV-1a continued from `state`.
#[must_use]
pub fn fnv1a_with(state: u64, bytes: &[u8]) -> u64 {
    let mut h = state;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_vector() {
        // The FNV-1a test vector for "a".
        assert_eq!(super::fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(super::fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }
}
