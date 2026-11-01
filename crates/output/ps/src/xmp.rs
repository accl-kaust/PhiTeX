//! The XMP packet's document identifier (gdevpdfe.c's `pdf_make_uuid`):
//! a version-1 UUID of gs's clock, its node from the digest gs keeps of
//! the file written so far.

use alloc::format;
use alloc::string::String;

/// `uuid:...` for time `t` (Unix seconds) and the digest `d`.
pub fn uuid(d: &[u8; 16], t: i64) -> String {
    let e = t as u64;
    let ticks =
        10_000_000u64 * e + e / 100 + 10_000_000u64 * 86400 * (17 + 30 + 31 + 365 * 397 + 99);
    let lo = ticks & 0xffff_ffff;
    let md = (ticks >> 32) & 0xffff;
    let hi = ((ticks >> 48) & 0x0fff) | 0x1000;
    format!(
        "uuid:{lo:08x}-{md:04x}-{hi:04x}-0000-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        d[0], d[1], d[2], d[3], d[4], d[5]
    )
}
