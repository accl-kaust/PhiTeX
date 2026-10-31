//! Part 19 of `mf.web`: generating discrete moves.

use crate::arith::{ab_vs_cd, half};
use crate::{Mf, R, Scaled, consts::*};

impl Mf {
    // §311
    pub(crate) fn make_moves(
        &mut self,
        xx0: Scaled,
        xx1: Scaled,
        xx2: Scaled,
        xx3: Scaled,
        yy0: Scaled,
        yy1: Scaled,
        yy2: Scaled,
        yy3: Scaled,
        xi_corr: i32,
        eta_corr: i32,
    ) -> R {
        if xx3 < xx0 || yy3 < yy0 {
            return self.confusion("m");
        }
        let mut l: i32 = 16;
        self.bisect_ptr = 0;
        let mut x1 = xx1 - xx0;
        let mut x2 = xx2 - xx1;
        let mut x3 = xx3 - xx2;
        let mut r = if xx0 >= xi_corr {
            (xx0 - xi_corr) % UNITY
        } else {
            UNITY - 1 - ((-xx0 + xi_corr - 1) % UNITY)
        };
        let mut m = (xx3 - xx0 + r) / UNITY;
        let mut y1 = yy1 - yy0;
        let mut y2 = yy2 - yy1;
        let mut y3 = yy3 - yy2;
        let mut s = if yy0 >= eta_corr {
            (yy0 - eta_corr) % UNITY
        } else {
            UNITY - 1 - ((-yy0 + eta_corr - 1) % UNITY)
        };
        let mut n = (yy3 - yy0 + s) / UNITY;
        if xx3 - xx0 >= FRACTION_ONE || yy3 - yy0 >= FRACTION_ONE {
            // §313
            x1 = half(x1 + xi_corr);
            x2 = half(x2 + xi_corr);
            x3 = half(x3 + xi_corr);
            r = half(r + xi_corr);
            y1 = half(y1 + eta_corr);
            y2 = half(y2 + eta_corr);
            y3 = half(y3 + eta_corr);
            s = half(s + eta_corr);
            l = 15;
        }
        loop {
            // §314
            if m == 0 {
                // §315
                while n > 0 {
                    self.move_ptr += 1;
                    self.move_[self.move_ptr as usize] = 1;
                    n -= 1;
                }
            } else if n == 0 {
                // §316
                self.move_[self.move_ptr as usize] += m;
            } else if m + n == 2 {
                // §317
                r = self.two_to_the[l as usize] - r;
                s = self.two_to_the[l as usize] - s;
                let mut done = false;
                while l < 30 {
                    let x3a = x3;
                    let x2a = half(x2 + x3 + xi_corr);
                    x2 = half(x1 + x2 + xi_corr);
                    x3 = half(x2 + x2a + xi_corr);
                    let t = x1 + x2 + x3;
                    r = r + r - xi_corr;
                    let y3a = y3;
                    let y2a = half(y2 + y3 + eta_corr);
                    y2 = half(y1 + y2 + eta_corr);
                    y3 = half(y2 + y2a + eta_corr);
                    let u = y1 + y2 + y3;
                    s = s + s - eta_corr;
                    if t < r {
                        if u < s {
                            // §318
                            x1 = x3;
                            x2 = x2a;
                            x3 = x3a;
                            r -= t;
                            y1 = y3;
                            y2 = y2a;
                            y3 = y3a;
                            s -= u;
                        } else {
                            self.move_up_then_right();
                            done = true;
                            break;
                        }
                    } else if u < s {
                        self.move_right_then_up();
                        done = true;
                        break;
                    }
                    l += 1;
                }
                if !done {
                    r -= xi_corr;
                    s -= eta_corr;
                    if ab_vs_cd(x1 + x2 + x3, s, y1 + y2 + y3, r) - xi_corr >= 0 {
                        self.move_right_then_up();
                    } else {
                        self.move_up_then_right();
                    }
                }
            } else {
                l += 1;
                let bp = self.bisect_ptr as usize;
                self.bisect_stack[bp + 10] = l;
                self.bisect_stack[bp + 2] = x3;
                self.bisect_stack[bp + 1] = half(x2 + x3 + xi_corr);
                x2 = half(x1 + x2 + xi_corr);
                x3 = half(x2 + self.bisect_stack[bp + 1] + xi_corr);
                self.bisect_stack[bp] = x3;
                r = r + r + xi_corr;
                let t = x1 + x2 + x3 + r;
                let q = t / self.two_to_the[l as usize];
                self.bisect_stack[bp + 3] = t % self.two_to_the[l as usize];
                self.bisect_stack[bp + 4] = m - q;
                m = q;
                self.bisect_stack[bp + 7] = y3;
                self.bisect_stack[bp + 6] = half(y2 + y3 + eta_corr);
                y2 = half(y1 + y2 + eta_corr);
                y3 = half(y2 + self.bisect_stack[bp + 6] + eta_corr);
                self.bisect_stack[bp + 5] = y3;
                s = s + s + eta_corr;
                let u = y1 + y2 + y3 + s;
                let q = u / self.two_to_the[l as usize];
                self.bisect_stack[bp + 8] = u % self.two_to_the[l as usize];
                self.bisect_stack[bp + 9] = n - q;
                n = q;
                self.bisect_ptr += MOVE_INCREMENT;
                continue;
            }
            if self.bisect_ptr == 0 {
                return Ok(());
            }
            // §312
            self.bisect_ptr -= MOVE_INCREMENT;
            let bp = self.bisect_ptr as usize;
            x1 = self.bisect_stack[bp];
            x2 = self.bisect_stack[bp + 1];
            x3 = self.bisect_stack[bp + 2];
            r = self.bisect_stack[bp + 3];
            m = self.bisect_stack[bp + 4];
            y1 = self.bisect_stack[bp + 5];
            y2 = self.bisect_stack[bp + 6];
            y3 = self.bisect_stack[bp + 7];
            s = self.bisect_stack[bp + 8];
            n = self.bisect_stack[bp + 9];
            l = self.bisect_stack[bp + 10];
        }
    }
    // §319
    pub(crate) fn move_right_then_up(&mut self) {
        self.move_[self.move_ptr as usize] += 1;
        self.move_ptr += 1;
        self.move_[self.move_ptr as usize] = 1;
    }
    // §320
    pub(crate) fn move_up_then_right(&mut self) {
        self.move_ptr += 1;
        self.move_[self.move_ptr as usize] = 2;
    }

    // §321
    pub(crate) fn smooth_moves(&mut self, b: i32, t: i32) {
        if t - b >= 3 {
            let mut k = b + 2;
            let mut aa = self.move_[(k - 1) as usize];
            let mut aaa = self.move_[(k - 2) as usize];
            loop {
                let a = self.move_[k as usize];
                if (a - aa).abs() > 1 {
                    // §322
                    if a > aa {
                        if aaa >= aa && a >= self.move_[(k + 1) as usize] {
                            self.move_[(k - 1) as usize] += 1;
                            self.move_[k as usize] = a - 1;
                        }
                    } else if aaa <= aa && a <= self.move_[(k + 1) as usize] {
                        self.move_[(k - 1) as usize] -= 1;
                        self.move_[k as usize] = a + 1;
                    }
                }
                k += 1;
                aaa = aa;
                aa = a;
                if k == t {
                    break;
                }
            }
        }
    }
}
