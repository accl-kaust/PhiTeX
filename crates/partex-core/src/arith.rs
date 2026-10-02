//! Part 7: Arithmetic with scaled dimensions (§99–§109).
//!
//! web2c's `integer` is a 32-bit `int`, so everything here is `i32`. Where
//! tex.web could overflow, the C build wraps in practice; `wrapping_*`
//! mirrors that.

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;
#[allow(unused_imports)]
pub use crate::web::{INF_BAD, TWO, UNITY};

/// §101: a scaled integer (multiple of 2^-16).
pub type Scaled = i32;

/// §109: glue ratios are C `double` in web2c (`GLUERATIO_TYPE`, texmfmp.h).
pub type GlueRatio = f64;

/// §100: half of an integer, rounding odd numbers up.
#[must_use]
pub fn half(x: i32) -> i32 {
    if x & 1 != 0 {
        x.wrapping_add(1) / 2
    } else {
        x / 2
    }
}

/// web2c's `zround` (lib/zround.c): Pascal `round`, clamped to TeX's range.
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

/// §108: badness of stretching or shrinking amounts summing to `s` to `t`.
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

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §102: convert the decimal fraction `.dig[0]dig[1]…dig[k-1]` to scaled.
    pub(crate) fn round_decimals(&self, mut k: usize) -> Scaled {
        let mut a: i32 = 0;
        while k > 0 {
            k -= 1;
            a = (a + i32::from(self.dig[k]) * TWO) / 10;
        }
        (a + 1) / 2
    }

    /// §103: print a scaled value, rounded to five digits.
    pub(crate) fn print_scaled(&mut self, mut s: Scaled) {
        if s < 0 {
            self.print_char(b'-');
            s = s.wrapping_neg();
        }
        self.print_int(s / UNITY);
        self.print_char(b'.');
        s = 10 * (s % UNITY) + 5;
        let mut delta: Scaled = 10;
        loop {
            if delta > UNITY {
                s += 0o100000 - 50000; // round the last digit
            }
            self.print_char(b'0' + u8::try_from(s / UNITY).unwrap_or(0));
            s = 10 * (s % UNITY);
            delta *= 10;
            if s <= delta {
                break;
            }
        }
    }

    /// §105: `n*x + y`, setting `arith_error` if the result would exceed
    /// `max_answer` in magnitude.
    pub(crate) fn mult_and_add(
        &mut self,
        mut n: i32,
        mut x: Scaled,
        y: Scaled,
        max_answer: Scaled,
    ) -> Scaled {
        if n < 0 {
            x = x.wrapping_neg();
            n = n.wrapping_neg();
        }
        if n == 0 {
            y
        } else if x <= max_answer.wrapping_sub(y) / n
            && x.wrapping_neg() <= max_answer.wrapping_add(y) / n
        {
            n.wrapping_mul(x).wrapping_add(y)
        } else {
            self.arith_error = true;
            0
        }
    }

    /// §105: `n*x + y` for scaled values.
    pub(crate) fn nx_plus_y(&mut self, n: i32, x: Scaled, y: Scaled) -> Scaled {
        self.mult_and_add(n, x, y, 0o7777777777)
    }

    /// §105: `n*x` for integers.
    pub(crate) fn mult_integers(&mut self, n: i32, x: i32) -> i32 {
        self.mult_and_add(n, x, 0, 0o17777777777)
    }

    /// §106: `x/n`, leaving the remainder in `remainder`.
    pub(crate) fn x_over_n(&mut self, mut x: Scaled, mut n: i32) -> Scaled {
        let mut negative = false;
        let result;
        if n == 0 {
            self.arith_error = true;
            result = 0;
            self.remainder = x;
        } else {
            if n < 0 {
                x = x.wrapping_neg();
                n = n.wrapping_neg();
                negative = true;
            }
            if x >= 0 {
                result = x / n;
                self.remainder = x % n;
            } else {
                result = -(x.wrapping_neg() / n);
                self.remainder = -(x.wrapping_neg() % n);
            }
        }
        if negative {
            self.remainder = self.remainder.wrapping_neg();
        }
        result
    }

    /// §107: `x*n/d` in 1.5-precision arithmetic, leaving the remainder in
    /// `remainder`. `n` and `d` are nonnegative and at most 2^16, `d > 0`.
    pub(crate) fn xn_over_d(&mut self, mut x: Scaled, n: i32, d: i32) -> Scaled {
        let positive = x >= 0;
        if !positive {
            x = x.wrapping_neg();
        }
        let t = (x % 0o100000).wrapping_mul(n);
        let mut u = (x / 0o100000).wrapping_mul(n).wrapping_add(t / 0o100000);
        let v = (u % d).wrapping_mul(0o100000).wrapping_add(t % 0o100000);
        if u / d >= 0o100000 {
            self.arith_error = true;
        } else {
            u = 0o100000 * (u / d) + (v / d);
        }
        if positive {
            self.remainder = v % d;
            u
        } else {
            self.remainder = -(v % d);
            -u
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{engine, term_output};

    #[test]
    fn half_and_badness() {
        assert_eq!([half(5), half(-5), half(4), half(-4)], [3, -2, 2, -2]);
        assert_eq!(badness(0, 0), 0);
        assert_eq!(badness(10, 0), INF_BAD);
        // Expected values from the oracle: `\badness` of
        // `\hbox to t{\hskip 0pt plus s}` in TeX Live's tex.
        assert_eq!(badness(UNITY, UNITY), 100);
        assert_eq!(badness(UNITY / 2, UNITY), 12);
        assert_eq!(badness(3 * UNITY, UNITY), 2698);
        assert_eq!(badness(5 * UNITY, UNITY), INF_BAD);
        assert_eq!(badness(UNITY, 3 * UNITY), 4);
    }

    #[test]
    fn zround_matches_web2c() {
        assert_eq!(
            [zround(0.5), zround(-0.5), zround(1.49), zround(-2.5)],
            [1, -1, 1, -3]
        );
        assert_eq!(zround(1e12), 2_147_483_647);
        assert_eq!(zround(-1e12), -2_147_483_647);
    }

    #[test]
    fn scaled_printing_round_trips() {
        let mut t = engine();
        let out = term_output(&mut t, |t| {
            // Oracle: `\the\dimen0` after `\dimen0=<s>sp` (plus 2^31-1).
            for s in [
                0,
                UNITY,
                -UNITY / 2,
                1,
                0x7FFF_FFFF,
                16_383 * UNITY + 0xFFFF,
            ] {
                t.print_scaled(s);
                t.print_char(b' ');
            }
        });
        assert_eq!(out, b"0.0 1.0 -0.5 0.00002 32767.99998 16383.99998 ");

        // round_decimals inverts print_scaled.
        for (digits, want) in [
            (&[5u8][..], UNITY / 2),
            (&[0, 0, 0, 0, 2], 1),
            (&[9, 9, 9, 9, 8], 0xFFFF),
        ] {
            t.dig[..digits.len()].copy_from_slice(digits);
            assert_eq!(t.round_decimals(digits.len()), want);
        }
    }

    #[test]
    fn division_and_multiplication() {
        let mut t = engine();
        assert_eq!(t.x_over_n(7, 2), 3);
        assert_eq!(t.remainder, 1);
        assert_eq!(t.x_over_n(-7, 2), -3);
        assert_eq!(t.remainder, -1);
        // §106 negates both operands, then negates the remainder back.
        assert_eq!(t.x_over_n(7, -2), -3);
        assert_eq!(t.remainder, 1);
        assert!(!t.arith_error);
        assert_eq!(t.x_over_n(7, 0), 0);
        assert!(t.arith_error);

        t.arith_error = false;
        // Oracle: `\number\dimen0` after `\dimen0=1in`, `-1in`, `1truecm`.
        assert_eq!(t.xn_over_d(UNITY, 7227, 100), 4_736_286);
        assert_eq!(t.xn_over_d(-UNITY, 7227, 100), -4_736_286);
        assert_eq!(t.xn_over_d(UNITY, 7227, 254), 1_864_679);
        assert_eq!(t.xn_over_d(-UNITY, 1, 3), -21845);
        assert_eq!(t.remainder, -1);
        assert_eq!(t.nx_plus_y(3, UNITY, 1), 3 * UNITY + 1);
        assert!(!t.arith_error);
        assert_eq!(t.nx_plus_y(0o10000, UNITY * 0o100, 0), 0);
        assert!(t.arith_error);
        t.arith_error = false;
        assert_eq!(t.mult_integers(-46341, 46341), 0);
        assert!(t.arith_error);
    }
}
