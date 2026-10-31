//! Parts 7 and 8 of `mf.web`: arithmetic with scaled numbers, algebraic
//! and transcendental functions. `make_fraction`, `take_fraction`,
//! `make_scaled` and `take_scaled` are Hobby's floating-point versions of
//! `lib/texmfmp.c` (web2c builds without FIXPT); `floor_scaled`,
//! `floor_unscaled`, `round_unscaled`, `round_fraction`, `half` and `halfp`
//! are `cpascal.h`'s macros.

use crate::{Mf, R, Scaled, consts::*};

/// `half` (cpascal.h, for MF).
#[inline]
pub(crate) fn half(i: i32) -> i32 {
    if i < 0 { (i + 1) >> 1 } else { i >> 1 }
}
/// `halfp` (cpascal.h).
#[inline]
pub(crate) fn halfp(i: i32) -> i32 {
    i >> 1
}
#[inline]
pub(crate) fn floor_unscaled(i: i32) -> i32 {
    i >> 16
}
#[inline]
pub(crate) fn floor_scaled(i: i32) -> i32 {
    i & -65536
}
#[inline]
pub(crate) fn round_unscaled(i: i32) -> i32 {
    ((i >> 15) + 1) >> 1
}
#[inline]
pub(crate) fn round_fraction(i: i32) -> i32 {
    ((i >> 11) + 1) >> 1
}
#[inline]
pub(crate) fn odd(i: i32) -> bool {
    i & 1 != 0
}

const ELGORDO: i32 = 0x7fff_ffff;
const TWEXP31: f64 = 2_147_483_648.0;
const TWEXP28: f64 = 268_435_456.0;
const TWEXP16: f64 = 65536.0;
const TWEXP_16: f64 = 1.0 / 65536.0;
const TWEXP_28: f64 = 1.0 / 268_435_456.0;

impl Mf {
    // §100
    pub(crate) fn slow_add(&mut self, x: i32, y: i32) -> i32 {
        if x >= 0 {
            if y <= EL_GORDO - x {
                x + y
            } else {
                self.arith_error = true;
                EL_GORDO
            }
        } else if -y <= EL_GORDO + x {
            x + y
        } else {
            self.arith_error = true;
            -EL_GORDO
        }
    }

    /// `ztakefraction`: about p*q/2^28.
    pub(crate) fn take_fraction(&mut self, p: i32, q: i32) -> i32 {
        self.take_by(p, q, TWEXP_28)
    }
    /// `ztakescaled`: about p*q/2^16.
    pub(crate) fn take_scaled(&mut self, p: i32, q: i32) -> i32 {
        self.take_by(p, q, TWEXP_16)
    }
    pub(crate) fn take_by(&mut self, p: i32, q: i32, scale: f64) -> i32 {
        let mut d = f64::from(p) * f64::from(q) * scale;
        let i;
        if (p ^ q) >= 0 {
            d += 0.5;
            if d >= TWEXP31 {
                if d != TWEXP31 || ((p & 0o77777) * (q & 0o77777)) & 0o40000 == 0 {
                    self.arith_error = true;
                }
                return ELGORDO;
            }
            let mut j = d as i32;
            if d == f64::from(j) && ((p & 0o77777) * (q & 0o77777)) & 0o40000 != 0 {
                j -= 1;
            }
            i = j;
        } else {
            d -= 0.5;
            if d <= -TWEXP31 {
                if d != -TWEXP31 || ((-(p & 0o77777)) * (q & 0o77777)) & 0o40000 == 0 {
                    self.arith_error = true;
                }
                return -ELGORDO;
            }
            let mut j = d as i32;
            if d == f64::from(j) && ((-(p & 0o77777)) * (q & 0o77777)) & 0o40000 != 0 {
                j += 1;
            }
            i = j;
        }
        i
    }
    /// `zmakefraction`: about 2^28*p/q.
    pub(crate) fn make_fraction(&mut self, p: i32, q: i32) -> i32 {
        self.make_by(p, q, TWEXP28)
    }
    /// `zmakescaled`: about 2^16*p/q.
    pub(crate) fn make_scaled(&mut self, p: i32, q: i32) -> i32 {
        self.make_by(p, q, TWEXP16)
    }
    pub(crate) fn make_by(&mut self, p: i32, q: i32, scale: f64) -> i32 {
        let mut d = scale * f64::from(p) / f64::from(q);
        if (p ^ q) >= 0 {
            d += 0.5;
            if d >= TWEXP31 {
                self.arith_error = true;
                return ELGORDO;
            }
            let mut i = d as i32;
            if d == f64::from(i)
                && (((if q > 0 { -q } else { q }) & 0o77777) * (((i & 0o37777) << 1) - 1)) & 0o4000
                    != 0
            {
                i -= 1;
            }
            i
        } else {
            d -= 0.5;
            if d <= -TWEXP31 {
                self.arith_error = true;
                return -ELGORDO;
            }
            let mut i = d as i32;
            if d == f64::from(i)
                && (((if q > 0 { q } else { -q }) & 0o77777) * (((i & 0o37777) << 1) + 1)) & 0o4000
                    != 0
            {
                i += 1;
            }
            i
        }
    }

    // §116
    pub(crate) fn velocity(&mut self, st: i32, ct: i32, sf: i32, cf: i32, t: Scaled) -> i32 {
        let mut acc = self.take_fraction(st - (sf / 16), sf - (st / 16));
        acc = self.take_fraction(acc, ct - cf);
        let mut num = FRACTION_TWO + self.take_fraction(acc, 379_625_062);
        let denom = FRACTION_THREE
            + self.take_fraction(ct, 497_706_707)
            + self.take_fraction(cf, 307_599_661);
        if t != UNITY {
            num = self.make_scaled(num, t);
        }
        if num / 4 >= denom {
            FRACTION_FOUR
        } else {
            self.make_fraction(num, denom)
        }
    }

    // §121
    pub(crate) fn square_rt(&mut self, mut x: Scaled) -> R<Scaled> {
        if x <= 0 {
            // §122
            if x < 0 {
                self.print_err("Square root of ");
                self.print_scaled(x);
                self.prints(" has been replaced by 0");
                self.help(&[
                    "Since I don't take square roots of negative numbers,",
                    "I'm zeroing this one. Proceed, with fingers crossed.",
                ]);
                self.error()?;
            }
            return Ok(0);
        }
        let mut k = 23;
        let mut q = 2;
        while x < FRACTION_TWO {
            k -= 1;
            x = x + x + x + x;
        }
        let mut y;
        if x < FRACTION_FOUR {
            y = 0;
        } else {
            x -= FRACTION_FOUR;
            y = 1;
        }
        loop {
            // §123
            x += x;
            y += y;
            if x >= FRACTION_FOUR {
                x -= FRACTION_FOUR;
                y += 1;
            }
            x += x;
            y = y + y - q;
            q += q;
            if x >= FRACTION_FOUR {
                x -= FRACTION_FOUR;
                y += 1;
            }
            if y > q {
                y -= q;
                q += 2;
            } else if y <= 0 {
                q -= 2;
                y += q;
            }
            k -= 1;
            if k == 0 {
                break;
            }
        }
        Ok(halfp(q))
    }

    // §124
    pub(crate) fn pyth_add(&mut self, a: i32, b: i32) -> i32 {
        let mut a = a.abs();
        let mut b = b.abs();
        if a < b {
            std::mem::swap(&mut a, &mut b);
        }
        if b > 0 {
            let big;
            if a < FRACTION_TWO {
                big = false;
            } else {
                a /= 4;
                b /= 4;
                big = true;
            }
            // §125
            loop {
                let mut r = self.make_fraction(b, a);
                r = self.take_fraction(r, r);
                if r == 0 {
                    break;
                }
                r = self.make_fraction(r, FRACTION_FOUR + r);
                a += self.take_fraction(a + a, r);
                b = self.take_fraction(b, r);
            }
            if big {
                if a < FRACTION_TWO {
                    a = a + a + a + a;
                } else {
                    self.arith_error = true;
                    a = EL_GORDO;
                }
            }
        }
        a
    }

    // §126
    pub(crate) fn pyth_sub(&mut self, a: i32, b: i32) -> R<i32> {
        let mut a = a.abs();
        let mut b = b.abs();
        if a <= b {
            // §128
            if a < b {
                self.print_err("Pythagorean subtraction ");
                self.print_scaled(a);
                self.prints("+-+");
                self.print_scaled(b);
                self.prints(" has been replaced by 0");
                self.help(&[
                    "Since I don't take square roots of negative numbers,",
                    "I'm zeroing this one. Proceed, with fingers crossed.",
                ]);
                self.error()?;
            }
            a = 0;
        } else {
            let big;
            if a < FRACTION_FOUR {
                big = false;
            } else {
                a = halfp(a);
                b = halfp(b);
                big = true;
            }
            // §127
            loop {
                let mut r = self.make_fraction(b, a);
                r = self.take_fraction(r, r);
                if r == 0 {
                    break;
                }
                r = self.make_fraction(r, FRACTION_FOUR - r);
                a -= self.take_fraction(a + a, r);
                b = self.take_fraction(b, r);
            }
            if big {
                a += a;
            }
        }
        Ok(a)
    }

    // §131
    pub(crate) fn init_arith_tables(&mut self) {
        self.two_to_the[0] = 1;
        for k in 1..=30 {
            self.two_to_the[k] = 2 * self.two_to_the[k - 1];
        }
        let sl = [
            93_032_640, 38_612_034, 17_922_280, 8_662_214, 4_261_238, 2_113_709, 1_052_693,
            525_315, 262_400, 131_136, 65552, 32772, 16385,
        ];
        for (k, v) in sl.iter().enumerate() {
            self.spec_log[k + 1] = *v;
        }
        for k in 14..=27 {
            self.spec_log[k] = self.two_to_the[27 - k];
        }
        self.spec_log[28] = 1;
        // §138
        let sa = [
            27_855_475, 14_718_068, 7_471_121, 3_750_058, 1_876_857, 938_658, 469_357, 234_682,
            117_342, 58671, 29335, 14668, 7334, 3667, 1833, 917, 458, 229, 115, 57, 29, 14, 7, 4,
            2, 1,
        ];
        for (k, v) in sa.iter().enumerate() {
            self.spec_atan[k + 1] = *v;
        }
    }

    // §132
    pub(crate) fn m_log(&mut self, mut x: Scaled) -> R<Scaled> {
        if x <= 0 {
            // §134
            self.print_err("Logarithm of ");
            self.print_scaled(x);
            self.prints(" has been replaced by 0");
            self.help(&[
                "Since I don't take logs of non-positive numbers,",
                "I'm zeroing this one. Proceed, with fingers crossed.",
            ]);
            self.error()?;
            return Ok(0);
        }
        let mut y: i32 = 1_302_456_956 + 4 - 100;
        let mut z: i32 = 27595 + 6_553_600;
        while x < FRACTION_FOUR {
            x += x;
            y -= 93_032_639;
            z -= 48782;
        }
        y += z / UNITY;
        let mut k = 2usize;
        while x > FRACTION_FOUR + 4 {
            // §133
            z = ((x - 1) / self.two_to_the[k]) + 1;
            while x < FRACTION_FOUR + z {
                z = halfp(z + 1);
                k += 1;
            }
            y += self.spec_log[k];
            x -= z;
        }
        Ok(y / 8)
    }

    // §135
    pub(crate) fn m_exp(&mut self, x: Scaled) -> Scaled {
        if x > 174_436_200 {
            self.arith_error = true;
            EL_GORDO
        } else if x < -197_694_359 {
            0
        } else {
            let mut y;
            let mut z;
            if x <= 0 {
                z = -8 * x;
                y = 0o4000000;
            } else {
                if x <= 127_919_879 {
                    z = 1_023_359_037 - 8 * x;
                } else {
                    z = 8 * (174_436_200 - x);
                }
                y = EL_GORDO;
            }
            // §136
            let mut k = 1usize;
            while z > 0 {
                while z >= self.spec_log[k] {
                    z -= self.spec_log[k];
                    y = y - 1 - ((y - self.two_to_the[k - 1]) / self.two_to_the[k]);
                }
                k += 1;
            }
            if x <= 127_919_879 { (y + 8) / 16 } else { y }
        }
    }

    // §139
    pub(crate) fn n_arg(&mut self, mut x: i32, mut y: i32) -> R<i32> {
        let mut octant;
        if x >= 0 {
            octant = FIRST_OCTANT;
        } else {
            x = -x;
            octant = FIRST_OCTANT + NEGATE_X;
        }
        if y < 0 {
            y = -y;
            octant += NEGATE_Y;
        }
        if x < y {
            std::mem::swap(&mut x, &mut y);
            octant += SWITCH_X_AND_Y;
        }
        if x == 0 {
            // §140
            self.print_err("angle(0,0) is taken as zero");
            self.help(&[
                "The `angle' between two identical points is undefined.",
                "I'm zeroing this one. Proceed, with fingers crossed.",
            ]);
            self.error()?;
            return Ok(0);
        }
        // §142
        while x >= FRACTION_TWO {
            x = halfp(x);
            y = halfp(y);
        }
        let mut z = 0;
        if y > 0 {
            while x < FRACTION_ONE {
                x += x;
                y += y;
            }
            // §143
            let mut k = 0usize;
            loop {
                y += y;
                k += 1;
                if y > x {
                    z += self.spec_atan[k];
                    let t = x;
                    x += y / self.two_to_the[k + k];
                    y -= t;
                }
                if k == 15 {
                    break;
                }
            }
            loop {
                y += y;
                k += 1;
                if y > x {
                    z += self.spec_atan[k];
                    y -= x;
                }
                if k == 26 {
                    break;
                }
            }
        }
        // §141
        Ok(match octant {
            FIRST_OCTANT => z,
            SECOND_OCTANT => NINETY_DEG - z,
            THIRD_OCTANT => NINETY_DEG + z,
            FOURTH_OCTANT => ONE_EIGHTY_DEG - z,
            FIFTH_OCTANT => z - ONE_EIGHTY_DEG,
            SIXTH_OCTANT => -z - NINETY_DEG,
            SEVENTH_OCTANT => z - NINETY_DEG,
            _ => -z,
        })
    }

    // §145
    pub(crate) fn n_sin_cos(&mut self, mut z: i32) {
        while z < 0 {
            z += THREE_SIXTY_DEG;
        }
        z %= THREE_SIXTY_DEG;
        let q = z / FORTY_FIVE_DEG;
        z %= FORTY_FIVE_DEG;
        let mut x = FRACTION_ONE;
        let mut y = x;
        if !odd(q) {
            z = FORTY_FIVE_DEG - z;
        }
        // §147
        let mut k = 1usize;
        while z > 0 {
            if z >= self.spec_atan[k] {
                z -= self.spec_atan[k];
                let t = x;
                x = t + y / self.two_to_the[k];
                y -= t / self.two_to_the[k];
            }
            k += 1;
        }
        if y < 0 {
            y = 0;
        }
        // §146
        match q {
            0 => {}
            1 => std::mem::swap(&mut x, &mut y),
            2 => {
                let t = x;
                x = -y;
                y = t;
            }
            3 => x = -x,
            4 => {
                x = -x;
                y = -y;
            }
            5 => {
                let t = x;
                x = -y;
                y = -t;
            }
            6 => {
                let t = x;
                x = y;
                y = -t;
            }
            _ => y = -y,
        }
        let r = self.pyth_add(x, y);
        self.n_cos = self.make_fraction(x, r);
        self.n_sin = self.make_fraction(y, r);
    }

    // §149
    pub(crate) fn new_randoms(&mut self) {
        for k in 0..=23 {
            let mut x = self.randoms[k] - self.randoms[k + 31];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.randoms[k] = x;
        }
        for k in 24..=54 {
            let mut x = self.randoms[k] - self.randoms[k - 24];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.randoms[k] = x;
        }
        self.j_random = 54;
    }
    #[inline]
    pub(crate) fn next_random(&mut self) {
        if self.j_random == 0 {
            self.new_randoms();
        } else {
            self.j_random -= 1;
        }
    }
    // §150
    pub(crate) fn init_randoms(&mut self, seed: Scaled) {
        let mut j = seed.abs();
        while j >= FRACTION_ONE {
            j = halfp(j);
        }
        let mut k = 1;
        for i in 0..=54 {
            let jj = k;
            k = j - k;
            j = jj;
            if k < 0 {
                k += FRACTION_ONE;
            }
            self.randoms[(i * 21) % 55] = j;
        }
        self.new_randoms();
        self.new_randoms();
        self.new_randoms();
    }
    // §151
    pub(crate) fn unif_rand(&mut self, x: Scaled) -> Scaled {
        self.next_random();
        let y = self.take_fraction(x.abs(), self.randoms[self.j_random as usize]);
        if y == x.abs() {
            0
        } else if x > 0 {
            y
        } else {
            -y
        }
    }
    // §152
    pub(crate) fn norm_rand(&mut self) -> R<Scaled> {
        let mut x;
        loop {
            let mut u;
            loop {
                self.next_random();
                x = self.take_fraction(
                    112_429,
                    self.randoms[self.j_random as usize] - FRACTION_HALF,
                );
                self.next_random();
                u = self.randoms[self.j_random as usize];
                if x.abs() < u {
                    break;
                }
            }
            x = self.make_fraction(x, u);
            let l = 139_548_960 - self.m_log(u)?;
            if ab_vs_cd(1024, l, x, x) >= 0 {
                break;
            }
        }
        Ok(x)
    }
}

// §117
pub(crate) fn ab_vs_cd(mut a: i32, mut b: i32, mut c: i32, mut d: i32) -> i32 {
    // §118
    if a < 0 {
        a = -a;
        b = -b;
    }
    if c < 0 {
        c = -c;
        d = -d;
    }
    if d <= 0 {
        if b >= 0 {
            return if (a == 0 || b == 0) && (c == 0 || d == 0) {
                0
            } else {
                1
            };
        }
        if d == 0 {
            return if a == 0 { 0 } else { -1 };
        }
        let q = a;
        a = c;
        c = q;
        let q = -b;
        b = -d;
        d = q;
    } else if b <= 0 {
        if b < 0 && a > 0 {
            return -1;
        }
        return if c == 0 { 0 } else { -1 };
    }
    loop {
        let q = a / d;
        let r = c / b;
        if q != r {
            return if q > r { 1 } else { -1 };
        }
        let q = a % d;
        let r = c % b;
        if r == 0 {
            return if q == 0 { 0 } else { 1 };
        }
        if q == 0 {
            return -1;
        }
        a = b;
        b = q;
        c = d;
        d = r;
    }
}
