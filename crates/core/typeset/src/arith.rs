//! Scaled arithmetic (tex.web §99–§110).
//!
//! Every function is exact integer arithmetic as tex.web specifies it. The
//! global `arith_error` and `remainder` of tex.web become return values.

/// A dimension in scaled points, `2^-16` pt (§101).
pub type Scaled = i32;

/// `2^16`, represents 1.0 (§101).
pub const UNITY: Scaled = 0o200000;
/// `2^17`, represents 2.0 (§101).
pub const TWO: Scaled = 0o400000;
/// Infinitely bad (§108).
pub const INF_BAD: i32 = 10000;
/// `2^30 - 1`, the largest legal dimension (§421).
pub const MAX_DIMEN: Scaled = 0o7777777777;

/// The result of a division: quotient, remainder, and whether tex.web's
/// `arith_error` would have been set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Division {
    /// The quotient (tex.web's function value).
    pub value: Scaled,
    /// tex.web's `remainder`.
    pub remainder: Scaled,
    /// tex.web's `arith_error:=true`.
    pub overflow: bool,
}

/// Half of an integer, odd values rounded up (§100).
#[must_use]
pub fn half(x: i32) -> i32 {
    if x & 1 != 0 { (x + 1) / 2 } else { x / 2 }
}

/// A decimal fraction `.d0 d1 ...` (at most 17 digits) as a correctly
/// rounded scaled value (§102).
#[must_use]
pub fn round_decimals(digits: &[u8]) -> Scaled {
    let mut a: i32 = 0;
    for &d in digits.iter().rev() {
        a = (a + i32::from(d) * TWO) / 10;
    }
    (a + 1) / 2
}

/// `n*x + y`, or `None` when the answer would exceed `max_answer` in
/// magnitude (§105, `arith_error`, value 0).
#[must_use]
pub fn mult_and_add(mut n: i32, mut x: Scaled, y: Scaled, max_answer: Scaled) -> Option<Scaled> {
    if n < 0 {
        x = -x;
        n = -n;
    }
    if n == 0 {
        Some(y)
    } else if x <= (max_answer - y) / n && -x <= (max_answer + y) / n {
        Some(n * x + y)
    } else {
        None
    }
}

/// `n*x + y` bounded by `2^30 - 1` (§105).
#[must_use]
pub fn nx_plus_y(n: i32, x: Scaled, y: Scaled) -> Option<Scaled> {
    mult_and_add(n, x, y, 0o7777777777)
}

/// `n*x` for integers bounded by `2^31 - 1` (§105).
#[must_use]
pub fn mult_integers(n: i32, x: i32) -> Option<i32> {
    mult_and_add(n, x, 0, 0o17777777777)
}

/// `x / n` truncated toward zero, with remainder (§106).
#[must_use]
pub fn x_over_n(x: Scaled, n: i32) -> Division {
    if n == 0 {
        return Division {
            value: 0,
            remainder: x,
            overflow: true,
        };
    }
    let (x, n, negative) = if n < 0 { (-x, -n, true) } else { (x, n, false) };
    let (value, mut remainder) = if x >= 0 {
        (x / n, x % n)
    } else {
        (-((-x) / n), -((-x) % n))
    };
    if negative {
        remainder = -remainder;
    }
    Division {
        value,
        remainder,
        overflow: false,
    }
}

/// `x * n / d` in 1.5-precision arithmetic, `0 <= n, d <= 2^16`, `d > 0`
/// (§107). On overflow the value is tex.web's (the unreduced `u`).
#[must_use]
pub fn xn_over_d(x: Scaled, n: i32, d: i32) -> Division {
    let positive = x >= 0;
    let x = if positive { x } else { -x };
    let t = (x % 0o100000) * n;
    let mut u = (x / 0o100000) * n + (t / 0o100000);
    let v = (u % d) * 0o100000 + (t % 0o100000);
    let overflow = u / d >= 0o100000;
    if !overflow {
        u = 0o100000 * (u / d) + (v / d);
    }
    if positive {
        Division {
            value: u,
            remainder: v % d,
            overflow,
        }
    } else {
        Division {
            value: -u,
            remainder: -(v % d),
            overflow,
        }
    }
}

/// Badness of making `t` from amounts summing to `s`, about `100(t/s)^3`
/// (§108); `t >= 0`.
#[must_use]
pub fn badness(t: Scaled, s: Scaled) -> i32 {
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
        (r * r * r + 0o400000) / 0o1000000
    }
}

/// `fract(x, n, d, max_answer)` (e-TeX, etex.ch): `x*n/d` rounded to
/// the nearest integer, halves away from zero; `None` when `d = 0` or the
/// magnitude would exceed `max_answer` (`num_error`, value 0).
#[must_use]
pub fn fract(x: i32, n: i32, d: i32, max_answer: i32) -> Option<i32> {
    if d == 0 {
        return None;
    }
    if x == 0 {
        return Some(0);
    }
    let negative = (d < 0) ^ (x < 0) ^ (n < 0);
    let (x, n, d) = (
        i128::from(x).abs(),
        i128::from(n).abs(),
        i128::from(d).abs(),
    );
    // floor(x n / d + 1/2)
    let a = (2 * x * n + d) / (2 * d);
    if a > i128::from(max_answer) {
        return None;
    }
    let a = i32::try_from(a).ok()?;
    Some(if negative { -a } else { a })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_rounds_odd_up() {
        assert_eq!(half(5), 3);
        assert_eq!(half(4), 2);
        assert_eq!(half(-5), -2);
        assert_eq!(half(-4), -2);
    }

    #[test]
    fn round_decimals_exact() {
        assert_eq!(round_decimals(&[5]), 32768);
        assert_eq!(round_decimals(&[1]), 6554);
        assert_eq!(round_decimals(&[9, 9, 9, 9, 9, 9]), UNITY);
        assert_eq!(round_decimals(&[]), 0);
    }

    #[test]
    fn mult_and_add_bounds() {
        assert_eq!(nx_plus_y(3, 10, 1), Some(31));
        assert_eq!(nx_plus_y(-3, 10, 1), Some(-29));
        assert_eq!(nx_plus_y(0, 10, 7), Some(7));
        assert_eq!(nx_plus_y(2, 0o4000000000, 0), None);
        assert_eq!(mult_integers(2, 0o4000000000), Some(0o10000000000));
    }

    #[test]
    fn x_over_n_signs() {
        assert_eq!(
            x_over_n(7, 2),
            Division {
                value: 3,
                remainder: 1,
                overflow: false
            }
        );
        assert_eq!(
            x_over_n(-7, 2),
            Division {
                value: -3,
                remainder: -1,
                overflow: false
            }
        );
        assert_eq!(
            x_over_n(7, -2),
            Division {
                value: -3,
                remainder: 1,
                overflow: false
            }
        );
        assert_eq!(
            x_over_n(-7, -2),
            Division {
                value: 3,
                remainder: -1,
                overflow: false
            }
        );
        assert!(x_over_n(7, 0).overflow);
    }

    #[test]
    fn xn_over_d_precise() {
        // 10pt * 1000/1000
        assert_eq!(xn_over_d(655_360, 1000, 1000).value, 655_360);
        // 1in = 72.27pt: 7227/100
        let d = xn_over_d(UNITY, 7227, 100);
        assert_eq!((d.value, d.remainder), (4_736_286, 72));
        let d = xn_over_d(-UNITY, 7227, 100);
        assert_eq!((d.value, d.remainder), (-4_736_286, -72));
        assert!(xn_over_d(0o7777777777, 0o200000, 1).overflow);
    }

    #[test]
    fn fract_rounds() {
        assert_eq!(fract(10, 1, 4, MAX_DIMEN), Some(3));
        assert_eq!(fract(-10, 1, 4, MAX_DIMEN), Some(-3));
        assert_eq!(fract(10, 1, -3, MAX_DIMEN), Some(-3));
        assert_eq!(fract(7, 0, 3, MAX_DIMEN), Some(0));
        assert_eq!(fract(1, 2, 0, MAX_DIMEN), None);
        assert_eq!(fract(MAX_DIMEN, 2, 1, MAX_DIMEN), None);
    }

    #[test]
    fn badness_values() {
        assert_eq!(badness(0, 0), 0);
        assert_eq!(badness(1, 0), INF_BAD);
        assert_eq!(badness(UNITY, UNITY), 100);
        assert_eq!(badness(UNITY / 2, UNITY), 12);
        assert_eq!(badness(2 * UNITY, UNITY), 800);
        assert_eq!(badness(5 * UNITY, UNITY), INF_BAD);
        assert_eq!(badness(10_000_000, 2_000_000), INF_BAD);
    }
}
