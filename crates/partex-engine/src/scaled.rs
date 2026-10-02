//! Fixed-point arithmetic: TeX's scaled points (tex.web part 7).

/// A dimension in scaled points (2^-16 pt).
pub type Scaled = i32;

/// §101: one point.
pub const UNITY: Scaled = 0o200000;
/// §108: `inf_bad`.
pub const INF_BAD: i32 = 10000;
/// §421: `max_dimen`, 2^30 - 1.
pub const MAX_DIMEN: Scaled = 0o7777777777;

/// §108: the badness of stretching or shrinking by `t` when `s` is
/// available, approximately 100(t/s)^3.
#[must_use]
pub fn badness(t: Scaled, s: Scaled) -> i32 {
    if t == 0 {
        0
    } else if s <= 0 {
        INF_BAD
    } else {
        let r = if t <= 7_230_584 {
            (t * 297) / s // 297^3 = 99.94 × 2^18
        } else if s >= 1_663_497 {
            t / (s / 297)
        } else {
            t
        };
        if r > 1290 {
            INF_BAD // 1290^3 < 2^31 < 1291^3
        } else {
            (r * r * r + 0o400000) / 0o1000000
        }
    }
}

/// §107: `x*n/d` rounded towards zero, for `x*n` up to 2^46 in magnitude;
/// `None` if the result does not fit in a scaled value.
#[must_use]
pub fn xn_over_d(x: Scaled, n: i32, d: i32) -> Option<Scaled> {
    let positive = x >= 0;
    let x = x.wrapping_abs();
    let t = (x % 0o100000).wrapping_mul(n);
    let u = (x / 0o100000).wrapping_mul(n).wrapping_add(t / 0o100000);
    let v = (u % d).wrapping_mul(0o100000).wrapping_add(t % 0o100000);
    if u / d >= 0o100000 {
        return None;
    }
    let u = 0o100000 * (u / d) + (v / d);
    Some(if positive { u } else { -u })
}

/// web2c's `zround` (lib/zround.c): Pascal `round`, clamped to TeX's
/// range.
#[must_use]
#[allow(clippy::cast_possible_truncation)] // the value is range-checked first
pub fn zround(r: f64) -> i32 {
    if r > 2_147_483_647.0 {
        2_147_483_647
    } else if r < -2_147_483_647.0 {
        -2_147_483_647
    } else if r >= 0.0 {
        (r + 0.5) as i32
    } else {
        (r - 0.5) as i32
    }
}

/// e-TeX's `fract`: `x*n/d` rounded, halves away from zero, if at most
/// `max` in absolute value.
pub fn fract(x: i32, n: i32, d: i32, max: i32, error: &mut bool) -> i32 {
    if d == 0 {
        *error = true;
        return 0;
    }
    if x == 0 {
        return 0;
    }
    let negative = ((x < 0) != (n < 0)) != (d < 0);
    let num = i128::from(x).abs() * i128::from(n).abs();
    let d = i128::from(d).abs();
    let a = (2 * num + d) / (2 * d);
    if a > i128::from(max) {
        *error = true;
        return 0;
    }
    let a = i32::try_from(a).unwrap_or(0);
    if negative { -a } else { a }
}

/// Powers of ten (pdfTeX's `ten_pow`).
pub const TEN_POW: [i32; 10] = [
    1,
    10,
    100,
    1000,
    10000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];

/// pdfTeX §689: `divide_scaled`: `(s / m) * 10^dd`, rounded; also the
/// part of `s` it accounts for (`scaled_out`). `None` if `m` is zero or
/// too big.
#[must_use]
pub fn divide_scaled(s: Scaled, m: Scaled, dd: i32) -> Option<(Scaled, Scaled)> {
    let mut sign = 1;
    let (mut s, mut m) = (s, m);
    if s < 0 {
        sign = -sign;
        s = -s;
    }
    if m < 0 {
        sign = -sign;
        m = -m;
    }
    if m == 0 || m >= i32::MAX / 10 {
        return None;
    }
    let mut q = s / m;
    let mut r = s % m;
    for _ in 0..dd {
        q = 10 * q + (10 * r) / m;
        r = (10 * r) % m;
    }
    if 2 * r >= m {
        q += 1;
        r -= m;
    }
    let out = sign * (s - (r / TEN_POW[usize::try_from(dd).unwrap_or(0)]));
    Some((sign * q, out))
}

/// pdfTeX §689: `round_xn_over_d`, as web2c computes it: in C `int`s,
/// so a negative `n` truncates toward zero instead of rounding.
#[must_use]
pub fn round_xn_over_d(x: Scaled, n: i32, d: i32) -> Scaled {
    let positive = x >= 0;
    let x = x.wrapping_abs();
    let t = (x % 0o100000).wrapping_mul(n);
    let mut u = (x / 0o100000).wrapping_mul(n).wrapping_add(t / 0o100000);
    let mut v = (u % d).wrapping_mul(0o100000).wrapping_add(t % 0o100000);
    if u / d >= 0o100000 {
        // (`arith_error`, never looked at)
    } else {
        u = (u / d).wrapping_mul(0o100000).wrapping_add(v / d);
    }
    v %= d;
    if 2 * v >= d {
        u += 1;
    }
    if positive { u } else { -u }
}

/// utils.c's `extxnoverd`: `x * n / d` rounded, in doubles.
#[expect(clippy::cast_possible_truncation, reason = "C's (scaled) r")]
#[must_use]
pub fn ext_xn_over_d(x: Scaled, n: i32, d: i32) -> Scaled {
    let mut r = f64::from(x) * f64::from(n) / f64::from(d);
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badness_matches_tex() {
        assert_eq!(badness(0, 0), 0);
        assert_eq!(badness(1, 0), INF_BAD);
        assert_eq!(badness(UNITY, UNITY), 100);
        assert_eq!(badness(UNITY, 2 * UNITY), 12); // (148^3 + 2^17) / 2^18
        assert_eq!(badness(10 * UNITY, UNITY), INF_BAD);
    }

    #[test]
    fn xn_over_d_scales_fonts() {
        // cmr10 scaled 1200
        assert_eq!(xn_over_d(10 * UNITY, 1200, 1000), Some(12 * UNITY));
        assert_eq!(xn_over_d(-3, 1, 2), Some(-1));
        assert_eq!(xn_over_d(MAX_DIMEN, 2000, 1), None);
    }
}
