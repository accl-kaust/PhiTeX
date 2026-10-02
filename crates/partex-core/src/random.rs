//! pdfTeX part 7b: Random numbers (pdfTeX §110–§127), taken from `MetaPost`.

use crate::arith::UNITY;
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// pdfTeX §111: $2^{27}$, $2^{28}$, $2^{30}$ and $2^{31}-1$ as fractions.
const FRACTION_HALF: i32 = 0o1000000000;
const FRACTION_ONE: i32 = 0o2000000000;
const FRACTION_FOUR: i32 = 0o10000000000;
const EL_GORDO: i32 = 0o17777777777;

/// pdfTeX §117–§118: $2^{27}\ln(1/(1-2^{-k}))$, rounded, for `m_log`.
const SPEC_LOG: [i32; 29] = [
    0, 93032640, 38612034, 17922280, 8662214, 4261238, 2113709, 1052693, 525315, 262400, 131136,
    65552, 32772, 16385, 8192, 4096, 2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1, 1,
];

/// pdfTeX §110: the generator's state.
#[derive(Clone, Debug, Hash)]
pub(crate) struct Randoms {
    /// The last 55 random values generated.
    values: [i32; 55],
    /// The number of unused `randoms`.
    j_random: usize,
    /// The default random seed (`\pdfrandomseed`).
    pub(crate) random_seed: i32,
}

partex_engine::persist_struct!(Randoms {
    values,
    j_random,
    random_seed
});

impl Default for Randoms {
    fn default() -> Self {
        Self {
            values: [0; 55],
            j_random: 0,
            random_seed: 0,
        }
    }
}

impl Randoms {
    /// pdfTeX §124.
    fn new_randoms(&mut self) {
        for k in 0..=23 {
            let mut x = self.values[k] - self.values[k + 31];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.values[k] = x;
        }
        for k in 24..=54 {
            let mut x = self.values[k] - self.values[k - 24];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.values[k] = x;
        }
        self.j_random = 54;
    }

    /// pdfTeX §124: `next_random`, then `randoms[j_random]`.
    fn next(&mut self) -> i32 {
        if self.j_random == 0 {
            self.new_randoms();
        } else {
            self.j_random -= 1;
        }
        self.values[self.j_random]
    }

    /// pdfTeX §125.
    pub(crate) fn init_randoms(&mut self, seed: i32) {
        let mut j = seed.wrapping_abs();
        while j >= FRACTION_ONE {
            j /= 2;
        }
        let mut k = 1;
        for i in 0..55 {
            let jj = k;
            k = j - k;
            j = jj;
            if k < 0 {
                k += FRACTION_ONE;
            }
            self.values[(i * 21) % 55] = j;
        }
        self.new_randoms();
        self.new_randoms();
        self.new_randoms(); // "warm up" the array
    }
}

/// pdfTeX §112: `make_frac(p, q)` $=\lfloor2^{28}p/q+{1\over2}\rfloor$.
fn make_frac(mut p: i32, mut q: i32, arith_error: &mut bool) -> i32 {
    let mut negative = false;
    if p < 0 {
        p = -p;
        negative = true;
    }
    if q <= 0 {
        q = -q;
        negative = !negative;
    }
    let n = p / q;
    p %= q;
    if n >= 8 {
        *arith_error = true;
        return if negative { -EL_GORDO } else { EL_GORDO };
    }
    let n = (n - 1) * FRACTION_ONE;
    // §113
    let mut f = 1;
    loop {
        let be_careful = p - q;
        p += be_careful;
        if p >= 0 {
            f = f + f + 1;
        } else {
            f += f;
            p += q;
        }
        if f >= FRACTION_ONE {
            break;
        }
    }
    if p - q + p >= 0 {
        f += 1;
    }
    if negative { -(f + n) } else { f + n }
}

/// pdfTeX §114: `take_frac(q, f)` $=\lfloor qf/2^{28}+{1\over2}\rfloor$.
#[allow(clippy::manual_midpoint)] // `halfp` as in the WEB source
fn take_frac(mut q: i32, mut f: i32, arith_error: &mut bool) -> i32 {
    // §115
    let mut negative = false;
    if f < 0 {
        f = -f;
        negative = true;
    }
    if q < 0 {
        q = -q;
        negative = !negative;
    }
    let mut n;
    if f < FRACTION_ONE {
        n = 0;
    } else {
        n = f / FRACTION_ONE;
        f %= FRACTION_ONE;
        if q <= EL_GORDO / n {
            n *= q;
        } else {
            *arith_error = true;
            n = EL_GORDO;
        }
    }
    f += FRACTION_ONE;
    // §116
    let mut p = FRACTION_HALF;
    if q < FRACTION_FOUR {
        loop {
            p = if f % 2 == 1 { (p + q) / 2 } else { p / 2 };
            f /= 2;
            if f == 1 {
                break;
            }
        }
    } else {
        loop {
            p = if f % 2 == 1 { p + (q - p) / 2 } else { p / 2 };
            f /= 2;
            if f == 1 {
                break;
            }
        }
    }
    if n - EL_GORDO + p > 0 {
        *arith_error = true;
        n = EL_GORDO - p;
    }
    if negative { -(n + p) } else { n + p }
}

/// pdfTeX §122: the sign of $ab-cd$.
fn ab_vs_cd(mut a: i32, mut b: i32, mut c: i32, mut d: i32) -> i32 {
    // §123
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
            return i32::from(!((a == 0 || b == 0) && (c == 0 || d == 0)));
        }
        if d == 0 {
            return if a == 0 { 0 } else { -1 };
        }
        core::mem::swap(&mut a, &mut c);
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
        let (q, r) = (a / d, c / b);
        if q != r {
            return if q > r { 1 } else { -1 };
        }
        let (q, r) = (a % d, c % b);
        if r == 0 {
            return i32::from(q != 0);
        }
        if q == 0 {
            return -1;
        }
        a = b;
        b = q;
        c = d;
        d = r;
    } // now `a>d>0` and `c>b>0`
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// pdfTeX §119: $2^{24}\ln x$.
    fn m_log(&mut self, mut x: i32) -> Result<i32, Jump> {
        if x <= 0 {
            // §121
            self.print_err(b"Logarithm of ");
            self.print_scaled(x);
            self.print_str(b" has been replaced by 0");
            self.help(&[
                b"Since I don't take logs of non-positive numbers,",
                b"I'm zeroing this one. Proceed, with fingers crossed.",
            ]);
            self.error()?;
            return Ok(0);
        }
        let mut y = 1302456956 + 4 - 100; // $14\times2^{27}\ln2$
        let mut z = 27595 + 6553600; // and $2^{16}\times .421063$
        while x < FRACTION_FOUR {
            x += x;
            y -= 93032639;
            z -= 48782;
        }
        y += z / UNITY;
        let mut k = 2;
        while x > FRACTION_FOUR + 4 {
            // §120
            let mut z = ((x - 1) >> k) + 1; // $z=\lceil x/2^k\rceil$
            while x < FRACTION_FOUR + z {
                z = (z + 1) / 2;
                k += 1;
            }
            y += SPEC_LOG[k];
            x -= z;
        }
        Ok(y / 8)
    }

    /// pdfTeX §126: uniform in $[0,x)$.
    pub(crate) fn unif_rand(&mut self, x: i32) -> i32 {
        self.random_read();
        if !T::VALUES {
            self.tracker.write(crate::track::Cell::Random);
        }
        let r = self.random.next();
        self.random_wrote();
        let y = take_frac(x.wrapping_abs(), r, &mut self.arith_error);
        if y == x.wrapping_abs() {
            0
        } else if x > 0 {
            y
        } else {
            -y
        }
    }

    /// pdfTeX §127: a normal deviate (times 65536).
    pub(crate) fn norm_rand(&mut self) -> Result<i32, Jump> {
        self.random_read();
        if !T::VALUES {
            self.tracker.write(crate::track::Cell::Random);
        }
        let r = self.norm_draw();
        self.random_wrote();
        r
    }

    /// §127's draws (the generator's state written by the caller).
    fn norm_draw(&mut self) -> Result<i32, Jump> {
        loop {
            let (mut x, mut u);
            loop {
                let r = self.random.next();
                x = take_frac(112429, r - FRACTION_HALF, &mut self.arith_error);
                u = self.random.next();
                if x.abs() < u {
                    break;
                }
            }
            x = make_frac(x, u, &mut self.arith_error);
            let l = 139548960 - self.m_log(u)?; // $2^{24}\cdot12\ln2$
            if ab_vs_cd(1024, l, x, x) >= 0 {
                return Ok(x);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frac_arithmetic() {
        let mut e = false;
        assert_eq!(make_frac(1, 2, &mut e), FRACTION_HALF);
        assert_eq!(take_frac(1000, FRACTION_HALF, &mut e), 500);
        assert_eq!(take_frac(-1000, FRACTION_ONE * 2, &mut e), -2000);
        assert!(!e);
        assert_eq!(ab_vs_cd(2, 3, 1, 6), 0);
        assert_eq!(ab_vs_cd(2, 4, 1, 6), 1);
        assert_eq!(ab_vs_cd(-2, 4, 1, 6), -1);
    }

    #[test]
    fn spec_log_table() {
        // §118: entries 14..=27 are powers of two, the rest are literal.
        for (k, &v) in SPEC_LOG.iter().enumerate().take(28).skip(14) {
            assert_eq!(v, 1 << (27 - k));
        }
    }
}
