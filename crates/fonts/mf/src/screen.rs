//! Part 27 of `mf.web`: online graphic output. The screen is web2c's
//! `window/trap.c` when `Config::trap_screen` is set (each call written to
//! the log file as the trap test expects), and no screen otherwise (as
//! web2c's `mf` without a display: `init_screen` is false).

use crate::arith::round_unscaled;
use crate::{Mf, Pointer, Scaled, consts::*};

impl Mf {
    // §566 (mf.ch, window/trap.c)
    fn init_screen(&mut self) -> bool {
        self.cfg.trap_screen
    }
    fn update_screen(&mut self) {
        if self.cfg.trap_screen {
            self.log_file.extend_from_slice(b"Calling UPDATESCREEN\n");
        }
    }
    fn blank_rectangle(&mut self, left: i32, right: i32, top: i32, bottom: i32) {
        if self.cfg.trap_screen {
            let s = format!("\nCalling BLANKRECTANGLE({left},{right},{top},{bottom})\n");
            self.log_file.extend_from_slice(s.as_bytes());
        }
    }
    fn paint_row(&mut self, r: i32, b: i32, n: i32) {
        if self.cfg.trap_screen {
            let mut s = format!("Calling PAINTROW({r},{b};");
            for k in 0..=n {
                s.push_str(&self.row_transition[k as usize].to_string());
                if k != n {
                    s.push(',');
                }
            }
            s.push_str(")\n");
            self.log_file.extend_from_slice(s.as_bytes());
        }
    }
    // §570
    fn start_screen(&mut self) {
        if !self.screen_started {
            self.screen_ok = self.init_screen();
            self.screen_started = true;
        }
    }

    // §572
    pub(crate) fn open_a_window(
        &mut self,
        k: usize,
        r0: Scaled,
        c0: Scaled,
        r1: Scaled,
        c1: Scaled,
        x: Scaled,
        y: Scaled,
    ) {
        // §573
        let mut r0 = if r0 < 0 { 0 } else { round_unscaled(r0) };
        let mut r1 = round_unscaled(r1);
        if r1 > self.screen_depth {
            r1 = self.screen_depth;
        }
        if r1 < r0 {
            if r0 > self.screen_depth {
                r0 = r1;
            } else {
                r1 = r0;
            }
        }
        let mut c0 = if c0 < 0 { 0 } else { round_unscaled(c0) };
        let mut c1 = round_unscaled(c1);
        if c1 > self.screen_width {
            c1 = self.screen_width;
        }
        if c1 < c0 {
            if c0 > self.screen_width {
                c0 = c1;
            } else {
                c1 = c0;
            }
        }
        self.window_open[k] = true;
        self.window_time[k] += 1;
        self.left_col[k] = c0;
        self.right_col[k] = c1;
        self.top_row[k] = r0;
        self.bot_row[k] = r1;
        // §574
        let m = round_unscaled(x);
        let n = round_unscaled(y) - 1;
        self.m_window[k] = c0 - m;
        self.n_window[k] = r0 + n;
        self.start_screen();
        if self.screen_ok {
            self.blank_rectangle(c0, c1, r0, r1);
            self.update_screen();
        }
    }

    // §577
    pub(crate) fn disp_edges(&mut self, k: usize) {
        if self.screen_ok
            && self.left_col[k] < self.right_col[k]
            && self.top_row[k] < self.bot_row[k]
        {
            let ce = self.cur_edges;
            let mut already_there = false;
            if self.last_window(ce) == k as i32 && self.last_window_time(ce) == self.window_time[k]
            {
                already_there = true;
            }
            if !already_there {
                self.blank_rectangle(
                    self.left_col[k],
                    self.right_col[k],
                    self.top_row[k],
                    self.bot_row[k],
                );
            }
            // §581
            let m_adjustment = self.m_window[k] - self.m_offset(ce);
            let right_edge = 8 * (self.right_col[k] - m_adjustment);
            let min_col = self.left_col[k];
            let mut p: Pointer = self.link(ce);
            let mut r = self.n_window[k] - (self.n_min(ce) - ZERO_FIELD);
            while p != ce && r >= self.top_row[k] {
                if r < self.bot_row[k] {
                    // §578
                    'done: {
                        if self.unsorted(p) > VOID {
                            self.sort_edges(p);
                        } else if self.unsorted(p) == VOID && already_there {
                            break 'done;
                        }
                        self.set_unsorted(p, VOID);
                        // §582
                        let mut n = 0;
                        let mut ww = 0;
                        let mut m = -1;
                        let mut w = 0;
                        let mut b = WHITE;
                        let mut q = self.sorted(p);
                        self.row_transition[0] = min_col;
                        loop {
                            let d = if q == self.sentinel() {
                                right_edge
                            } else {
                                self.info(q)
                            };
                            let mm = (d / 8) + m_adjustment;
                            if mm != m {
                                // §583
                                if w <= 0 {
                                    if ww > 0 && m > min_col {
                                        if n == 0 {
                                            if already_there {
                                                b = WHITE;
                                                n += 1;
                                            } else {
                                                b = BLACK;
                                            }
                                        } else {
                                            n += 1;
                                        }
                                        self.row_transition[n as usize] = m;
                                    }
                                } else if ww <= 0 && m > min_col {
                                    if n == 0 {
                                        b = BLACK;
                                    }
                                    n += 1;
                                    self.row_transition[n as usize] = m;
                                }
                                m = mm;
                                w = ww;
                            }
                            if d >= right_edge {
                                break;
                            }
                            ww = ww + (d % 8) - ZERO_W;
                            q = self.link(q);
                        }
                        // §584
                        if already_there || ww > 0 {
                            if n == 0 {
                                b = if ww > 0 { BLACK } else { WHITE };
                            }
                            n += 1;
                            self.row_transition[n as usize] = self.right_col[k];
                        } else if n == 0 {
                            break 'done;
                        }
                        self.paint_row(r, b, n);
                    }
                }
                p = self.link(p);
                r -= 1;
            }
            self.update_screen();
            self.window_time[k] += 1;
            self.set_last_window(ce, k as i32);
            self.set_last_window_time(ce, self.window_time[k]);
        }
    }
}
