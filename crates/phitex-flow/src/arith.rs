//! TeX's integer arithmetic (tex.web part 7), copied as pure functions.

pub const INF_BAD: i32 = 10000;
pub const AWFUL_BAD: i32 = 0o7777_777_777;

/// The badness of stretching or shrinking by `t` when `s` is available
/// (tex.web 108).
pub fn badness(t: i32, s: i32) -> i32 {
    if t == 0 {
        return 0;
    }
    if s <= 0 {
        return INF_BAD;
    }
    let r = if t <= 7_230_584 {
        (t * 297) / s
    } else if s >= 1_663_497 {
        t / (s / 297)
    } else {
        t
    };
    if r > 1290 {
        INF_BAD
    } else {
        (r * r * r + 0o400_000) / 0o1_000_000
    }
}

/// `x * n / d` exactly, and the remainder (tex.web 107); the sign follows
/// `x`.
pub fn xn_over_d(x: i32, n: i32, d: i32) -> (i32, i32) {
    let p = i64::from(x).abs() * i64::from(n);
    let (q, r) = (p / i64::from(d), p % i64::from(d));
    let q = q.min(i64::from(i32::MAX)) as i32;
    if x < 0 { (-q, -(r as i32)) } else { (q, r as i32) }
}

/// The fraction `0.d1d2...` in units of 2^-16, rounded (tex.web 102).
pub fn round_decimals(digits: &[u8]) -> i32 {
    let mut a: i32 = 0;
    for &d in digits.iter().rev() {
        a = (a + i32::from(d) * 0o400_000) / 10;
    }
    (a + 1) / 2
}

/// Clamps an `i64` into `i32`.
pub fn clamp(x: i64) -> i32 {
    x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
