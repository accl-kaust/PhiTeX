//! Part 20 of `mf.web`: edge structures.

use crate::arith::{ab_vs_cd, round_unscaled};
use crate::{Mf, Pointer, R, Scaled, consts::*};

pub(crate) fn valid_range(x: i32) -> bool {
    (x - 4096).abs() < 4096
}

impl Mf {
    #[inline]
    pub(crate) fn empty_edges(&self, h: Pointer) -> bool {
        self.link(h) == h
    }

    // §326
    pub(crate) fn init_edges(&mut self, h: Pointer) {
        self.set_knil(h, h);
        self.set_link(h, h);
        self.set_n_min(h, ZERO_FIELD + 4095);
        self.set_n_max(h, ZERO_FIELD - 4095);
        self.set_m_min(h, ZERO_FIELD + 4095);
        self.set_m_max(h, ZERO_FIELD - 4095);
        self.set_m_offset(h, ZERO_FIELD);
        self.set_last_window(h, 0);
        self.set_last_window_time(h, 0);
        self.set_n_rover(h, h);
        self.set_n_pos(h, 0);
    }

    // §328
    pub(crate) fn fix_offset(&mut self) {
        let ce = self.cur_edges;
        let delta = 8 * (self.m_offset(ce) - ZERO_FIELD);
        self.set_m_offset(ce, ZERO_FIELD);
        let mut q = self.link(ce);
        while q != ce {
            let mut p = self.sorted(q);
            while p != self.sentinel() {
                let v = self.info(p) - delta;
                self.set_info(p, v);
                p = self.link(p);
            }
            p = self.unsorted(q);
            while p > VOID {
                let v = self.info(p) - delta;
                self.set_info(p, v);
                p = self.link(p);
            }
            q = self.link(q);
        }
    }

    // §329
    pub(crate) fn edge_prep(&mut self, ml: i32, mr: i32, nl: i32, nr: i32) -> R {
        let ce = self.cur_edges;
        let ml = ml + ZERO_FIELD;
        let mr = mr + ZERO_FIELD;
        let nl = nl + ZERO_FIELD;
        let nr = nr - 1 + ZERO_FIELD;
        if ml < self.m_min(ce) {
            self.set_m_min(ce, ml);
        }
        if mr > self.m_max(ce) {
            self.set_m_max(ce, mr);
        }
        let temp = self.m_offset(ce) - ZERO_FIELD;
        if !valid_range(self.m_min(ce) + temp) || !valid_range(self.m_max(ce) + temp) {
            self.fix_offset();
        }
        if self.empty_edges(ce) {
            self.set_n_min(ce, nr + 1);
            self.set_n_max(ce, nr);
        }
        if nl < self.n_min(ce) {
            // §330
            let mut delta = self.n_min(ce) - nl;
            self.set_n_min(ce, nl);
            let mut p = self.link(ce);
            loop {
                let q = self.get_node(ROW_NODE_SIZE)?;
                self.set_sorted(q, self.sentinel());
                self.set_unsorted(q, VOID);
                self.set_knil(p, q);
                self.set_link(q, p);
                p = q;
                delta -= 1;
                if delta == 0 {
                    break;
                }
            }
            self.set_knil(p, ce);
            self.set_link(ce, p);
            if self.n_rover(ce) == ce {
                self.set_n_pos(ce, nl - 1);
            }
        }
        if nr > self.n_max(ce) {
            // §331
            let mut delta = nr - self.n_max(ce);
            self.set_n_max(ce, nr);
            let mut p = self.knil(ce);
            loop {
                let q = self.get_node(ROW_NODE_SIZE)?;
                self.set_sorted(q, self.sentinel());
                self.set_unsorted(q, VOID);
                self.set_link(p, q);
                self.set_knil(q, p);
                p = q;
                delta -= 1;
                if delta == 0 {
                    break;
                }
            }
            self.set_link(p, ce);
            self.set_knil(ce, p);
            if self.n_rover(ce) == ce {
                self.set_n_pos(ce, nr + 1);
            }
        }
        Ok(())
    }

    // §332
    pub(crate) fn print_edges(&mut self, s: &str, nuline: bool, x_off: i32, y_off: i32) {
        self.print_diagnostic("Edge structure", s, nuline);
        let ce = self.cur_edges;
        let mut p = self.knil(ce);
        let mut n = self.n_max(ce) - ZERO_FIELD;
        while p != ce {
            let mut q = self.unsorted(p);
            let mut r = self.sorted(p);
            if q > VOID || r != self.sentinel() {
                self.print_nl("row ");
                self.print_int(n + y_off);
                self.print_char(b':');
                while q > VOID {
                    self.print_weight(q, x_off);
                    q = self.link(q);
                }
                self.prints(" |");
                while r != self.sentinel() {
                    self.print_weight(r, x_off);
                    r = self.link(r);
                }
            }
            p = self.knil(p);
            n -= 1;
        }
        self.end_diagnostic(true);
    }

    // §333
    pub(crate) fn print_weight(&mut self, q: Pointer, x_off: i32) {
        let d = self.info(q);
        let mut w = d % 8;
        let m = (d / 8) - self.m_offset(self.cur_edges);
        if self.file_offset > self.max_print_line - 9 {
            self.print_nl(" ");
        } else {
            self.print_char(b' ');
        }
        self.print_int(m + x_off);
        while w > ZERO_W {
            self.print_char(b'+');
            w -= 1;
        }
        while w < ZERO_W {
            self.print_char(b'-');
            w += 1;
        }
    }

    // §334
    pub(crate) fn copy_edges(&mut self, h: Pointer) -> R<Pointer> {
        let hh = self.get_node(EDGE_HEADER_SIZE)?;
        for k in 1..=4 {
            self.mem[(hh + k) as usize] = self.mem[(h + k) as usize];
        }
        self.set_n_pos(hh, self.n_max(hh) + 1);
        self.set_n_rover(hh, hh);
        let mut p = self.link(h);
        let mut qq = hh;
        while p != h {
            let pp = self.get_node(ROW_NODE_SIZE)?;
            self.set_link(qq, pp);
            self.set_knil(pp, qq);
            self.copy_rows(p, pp)?;
            p = self.link(p);
            qq = pp;
        }
        self.set_link(qq, hh);
        self.set_knil(hh, qq);
        Ok(hh)
    }
    // §335
    pub(crate) fn copy_rows(&mut self, p: Pointer, pp: Pointer) -> R {
        let mut r = self.sorted(p);
        let mut rr = pp + 1;
        while r != self.sentinel() {
            let ss = self.get_avail()?;
            self.set_link(rr, ss);
            rr = ss;
            self.set_info(rr, self.info(r));
            r = self.link(r);
        }
        self.set_link(rr, self.sentinel());
        r = self.unsorted(p);
        rr = self.temp_head();
        while r > VOID {
            let ss = self.get_avail()?;
            self.set_link(rr, ss);
            rr = ss;
            self.set_info(rr, self.info(r));
            r = self.link(r);
        }
        self.set_link(rr, r);
        self.set_unsorted(pp, self.link(self.temp_head()));
        Ok(())
    }

    // §336
    pub(crate) fn y_reflect_edges(&mut self) {
        let ce = self.cur_edges;
        let mut p = self.n_min(ce);
        self.set_n_min(ce, ZERO_FIELD + ZERO_FIELD - 1 - self.n_max(ce));
        self.set_n_max(ce, ZERO_FIELD + ZERO_FIELD - 1 - p);
        self.set_n_pos(ce, ZERO_FIELD + ZERO_FIELD - 1 - self.n_pos(ce));
        p = self.link(ce);
        let mut q = ce;
        loop {
            let r = self.link(p);
            self.set_link(p, q);
            self.set_knil(q, p);
            q = p;
            p = r;
            if q == ce {
                break;
            }
        }
        self.set_last_window_time(ce, 0);
    }

    // §337
    pub(crate) fn x_reflect_edges(&mut self) {
        let ce = self.cur_edges;
        let mut p = self.m_min(ce);
        self.set_m_min(ce, ZERO_FIELD + ZERO_FIELD - self.m_max(ce));
        self.set_m_max(ce, ZERO_FIELD + ZERO_FIELD - p);
        let m =
            (ZERO_FIELD + self.m_offset(ce)) * 8 + ZERO_W + MIN_HALFWORD + ZERO_W + MIN_HALFWORD;
        self.set_m_offset(ce, ZERO_FIELD);
        p = self.link(ce);
        loop {
            // §339
            let mut q = self.sorted(p);
            let mut r = self.sentinel();
            while q != self.sentinel() {
                let s = self.link(q);
                self.set_link(q, r);
                r = q;
                self.set_info(r, m - self.info(q));
                q = s;
            }
            self.set_sorted(p, r);
            // §338
            q = self.unsorted(p);
            while q > VOID {
                self.set_info(q, m - self.info(q));
                q = self.link(q);
            }
            p = self.link(p);
            if p == ce {
                break;
            }
        }
        self.set_last_window_time(ce, 0);
    }

    // §340
    pub(crate) fn y_scale_edges(&mut self, s: i32) -> R {
        let ce = self.cur_edges;
        if s * (self.n_max(ce) + 1 - ZERO_FIELD) >= 4096
            || s * (self.n_min(ce) - ZERO_FIELD) <= -4096
        {
            self.print_err("Scaled picture would be too big");
            self.help(&[
                "I can't yscale the picture as requested---it would",
                "make some coordinates too large or too small.",
                "Proceed, and I'll omit the transformation.",
            ]);
            self.put_get_error()?;
        } else {
            self.set_n_max(ce, s * (self.n_max(ce) + 1 - ZERO_FIELD) - 1 + ZERO_FIELD);
            self.set_n_min(ce, s * (self.n_min(ce) - ZERO_FIELD) + ZERO_FIELD);
            // §341
            let mut p = ce;
            loop {
                let mut q = p;
                p = self.link(p);
                for _t in 2..=s {
                    let pp = self.get_node(ROW_NODE_SIZE)?;
                    self.set_link(q, pp);
                    self.set_knil(p, pp);
                    self.set_link(pp, p);
                    self.set_knil(pp, q);
                    q = pp;
                    self.copy_rows(p, pp)?;
                }
                if self.link(p) == ce {
                    break;
                }
            }
            self.set_last_window_time(ce, 0);
        }
        Ok(())
    }

    // §342
    pub(crate) fn x_scale_edges(&mut self, s: i32) -> R {
        let ce = self.cur_edges;
        if s * (self.m_max(ce) - ZERO_FIELD) >= 4096 || s * (self.m_min(ce) - ZERO_FIELD) <= -4096 {
            self.print_err("Scaled picture would be too big");
            self.help(&[
                "I can't xscale the picture as requested---it would",
                "make some coordinates too large or too small.",
                "Proceed, and I'll omit the transformation.",
            ]);
            self.put_get_error()?;
        } else if self.m_max(ce) != ZERO_FIELD || self.m_min(ce) != ZERO_FIELD {
            self.set_m_max(ce, s * (self.m_max(ce) - ZERO_FIELD) + ZERO_FIELD);
            self.set_m_min(ce, s * (self.m_min(ce) - ZERO_FIELD) + ZERO_FIELD);
            let delta = 8 * (ZERO_FIELD - s * self.m_offset(ce)) + MIN_HALFWORD;
            self.set_m_offset(ce, ZERO_FIELD);
            // §343
            let mut q = self.link(ce);
            loop {
                let mut p = self.sorted(q);
                while p != self.sentinel() {
                    let t = self.info(p);
                    let w = t % 8;
                    self.set_info(p, (t - w) * s + w + delta);
                    p = self.link(p);
                }
                p = self.unsorted(q);
                while p > VOID {
                    let t = self.info(p);
                    let w = t % 8;
                    self.set_info(p, (t - w) * s + w + delta);
                    p = self.link(p);
                }
                q = self.link(q);
                if q == ce {
                    break;
                }
            }
            self.set_last_window_time(ce, 0);
        }
        Ok(())
    }

    // §344
    pub(crate) fn negate_edges(&mut self, h: Pointer) {
        let mut p = self.link(h);
        while p != h {
            let mut q = self.unsorted(p);
            while q > VOID {
                let v = 8 - 2 * (self.info(q) % 8) + self.info(q);
                self.set_info(q, v);
                q = self.link(q);
            }
            q = self.sorted(p);
            if q != self.sentinel() {
                loop {
                    let v = 8 - 2 * (self.info(q) % 8) + self.info(q);
                    self.set_info(q, v);
                    q = self.link(q);
                    if q == self.sentinel() {
                        break;
                    }
                }
                // §345
                let mut u = p + 1;
                let mut q = self.link(u);
                let mut r = q;
                let mut s = self.link(r);
                loop {
                    if self.info(s) > self.info(r) {
                        self.set_link(u, q);
                        if s == self.sentinel() {
                            break;
                        }
                        u = r;
                        q = s;
                        r = q;
                        s = self.link(r);
                    } else {
                        let t = s;
                        s = self.link(t);
                        self.set_link(t, q);
                        q = t;
                    }
                }
                self.set_link(r, self.sentinel());
            }
            p = self.link(p);
        }
        self.set_last_window_time(h, 0);
    }

    // §346
    pub(crate) fn sort_edges(&mut self, h: Pointer) {
        let mut r = self.unsorted(h);
        self.set_unsorted(h, NULL);
        let mut p = self.link(r);
        self.set_link(r, self.sentinel());
        let th = self.temp_head();
        self.set_link(th, r);
        while p > VOID {
            let k = self.info(p);
            let mut q = th;
            loop {
                r = q;
                q = self.link(r);
                if k <= self.info(q) {
                    break;
                }
            }
            self.set_link(r, p);
            r = self.link(p);
            self.set_link(p, q);
            p = r;
        }
        // §347
        r = h + 1;
        let mut q = self.link(r);
        p = self.link(th);
        loop {
            let k = self.info(p);
            while k > self.info(q) {
                r = q;
                q = self.link(r);
            }
            self.set_link(r, p);
            let s = self.link(p);
            self.set_link(p, q);
            if s == self.sentinel() {
                break;
            }
            r = p;
            p = s;
        }
    }

    // §348
    pub(crate) fn cull_edges(&mut self, w_lo: i32, w_hi: i32, w_out: i32, w_in: i32) -> R {
        let ce = self.cur_edges;
        let mut min_d = MAX_HALFWORD;
        let mut max_d = MIN_HALFWORD;
        let mut min_n = MAX_HALFWORD;
        let mut max_n = MIN_HALFWORD;
        let mut p = self.link(ce);
        let mut n = self.n_min(ce);
        while p != ce {
            if self.unsorted(p) > VOID {
                self.sort_edges(p);
            }
            if self.sorted(p) != self.sentinel() {
                // §349
                let th = self.temp_head();
                let mut r = th;
                let mut q = self.sorted(p);
                let mut ww = 0;
                let mut m = 1_000_000;
                let mut prev_w = 0;
                let mut w = 0;
                loop {
                    let mm;
                    if q == self.sentinel() {
                        mm = 1_000_000;
                    } else {
                        let d = self.info(q);
                        mm = d / 8;
                        ww = ww + (d % 8) - ZERO_W;
                    }
                    if mm > m {
                        // §350
                        if w != prev_w {
                            let s = self.get_avail()?;
                            self.set_link(r, s);
                            self.set_info(s, 8 * m + MIN_HALFWORD + ZERO_W + w - prev_w);
                            r = s;
                            prev_w = w;
                        }
                        if q == self.sentinel() {
                            break;
                        }
                    }
                    m = mm;
                    if ww >= w_lo {
                        if ww <= w_hi {
                            w = w_in;
                        } else {
                            w = w_out;
                        }
                    } else {
                        w = w_out;
                    }
                    let s = self.link(q);
                    self.free_avail(q);
                    q = s;
                }
                self.set_link(r, self.sentinel());
                self.set_sorted(p, self.link(th));
                if r != th {
                    // §351
                    if min_n == MAX_HALFWORD {
                        min_n = n;
                    }
                    max_n = n;
                    if min_d > self.info(self.link(th)) {
                        min_d = self.info(self.link(th));
                    }
                    if max_d < self.info(r) {
                        max_d = self.info(r);
                    }
                }
            }
            p = self.link(p);
            n += 1;
        }
        // §352
        if min_n > max_n {
            // §353
            p = self.link(ce);
            while p != ce {
                let q = self.link(p);
                self.free_node(p, ROW_NODE_SIZE);
                p = q;
            }
            self.init_edges(ce);
        } else {
            n = self.n_min(ce);
            self.set_n_min(ce, min_n);
            while min_n > n {
                p = self.link(ce);
                self.set_link(ce, self.link(p));
                self.set_knil(self.link(p), ce);
                self.free_node(p, ROW_NODE_SIZE);
                n += 1;
            }
            n = self.n_max(ce);
            self.set_n_max(ce, max_n);
            self.set_n_pos(ce, max_n + 1);
            self.set_n_rover(ce, ce);
            while max_n < n {
                p = self.knil(ce);
                self.set_knil(ce, self.knil(p));
                self.set_link(self.knil(p), ce);
                self.free_node(p, ROW_NODE_SIZE);
                n -= 1;
            }
            self.set_m_min(ce, (min_d / 8) - self.m_offset(ce) + ZERO_FIELD);
            self.set_m_max(ce, (max_d / 8) - self.m_offset(ce) + ZERO_FIELD);
        }
        self.set_last_window_time(ce, 0);
        Ok(())
    }

    // §354
    pub(crate) fn xy_swap_edges(&mut self) -> R {
        let ce = self.cur_edges;
        // §356
        let m_spread = self.m_max(ce) - self.m_min(ce);
        if m_spread > MOVE_SIZE {
            return self.overflow("move table size", MOVE_SIZE);
        }
        for j in 0..=m_spread {
            self.move_[j as usize] = self.sentinel();
        }
        // §355
        let mut p = self.get_node(ROW_NODE_SIZE)?;
        self.set_sorted(p, self.sentinel());
        self.set_unsorted(p, NULL);
        self.set_knil(p, ce);
        self.set_knil(self.link(ce), p);
        p = self.get_node(ROW_NODE_SIZE)?;
        self.set_sorted(p, self.sentinel());
        self.set_knil(p, self.knil(ce));
        // §365
        let m_magic = self.m_min(ce) + self.m_offset(ce) - ZERO_FIELD;
        let mut n_magic = 8 * self.n_max(ce) + 8 + ZERO_W + MIN_HALFWORD;
        let mut m = 0;
        loop {
            let q = self.knil(p);
            if self.unsorted(q) > VOID {
                self.sort_edges(q);
            }
            // §358
            let mut r = self.sorted(p);
            self.free_node(p, ROW_NODE_SIZE);
            p = r;
            let mut pd = self.info(p);
            let mut pm = pd / 8;
            r = self.sorted(q);
            let mut rd = self.info(r);
            let mut rm = rd / 8;
            let mut w: i32 = 0;
            loop {
                let mm = if pm < rm { pm } else { rm };
                if w != 0 {
                    // §362
                    if m != mm {
                        if mm - m_magic >= MOVE_SIZE {
                            return self.confusion("xy");
                        }
                        let extras = (w.abs() - 1) / 3;
                        let mut xw = 0;
                        let ww;
                        if extras > 0 {
                            xw = if w > 0 { 3 } else { -3 };
                            ww = w - extras * xw;
                        } else {
                            ww = w;
                        }
                        loop {
                            let j = (m - m_magic) as usize;
                            for _k in 1..=extras {
                                let s = self.get_avail()?;
                                self.set_info(s, n_magic + xw);
                                self.set_link(s, self.move_[j]);
                                self.move_[j] = s;
                            }
                            let s = self.get_avail()?;
                            self.set_info(s, n_magic + ww);
                            self.set_link(s, self.move_[j]);
                            self.move_[j] = s;
                            m += 1;
                            if m == mm {
                                break;
                            }
                        }
                    }
                }
                let dw;
                if pd < rd {
                    dw = (pd % 8) - ZERO_W;
                    // §360
                    let s = self.link(p);
                    self.free_avail(p);
                    p = s;
                    pd = self.info(p);
                    pm = pd / 8;
                } else {
                    if r == self.sentinel() {
                        break;
                    }
                    dw = -((rd % 8) - ZERO_W);
                    // §359
                    r = self.link(r);
                    rd = self.info(r);
                    rm = rd / 8;
                }
                m = mm;
                w += dw;
            }
            p = q;
            n_magic -= 8;
            if self.knil(p) == ce {
                break;
            }
        }
        self.free_node(p, ROW_NODE_SIZE);
        // §364
        self.move_[m_spread as usize] = 0;
        let mut j = 0usize;
        while self.move_[j] == self.sentinel() {
            j += 1;
        }
        if j as i32 == m_spread {
            self.init_edges(ce);
        } else {
            let mm = self.m_min(ce);
            self.set_m_min(ce, self.n_min(ce));
            self.set_m_max(ce, self.n_max(ce) + 1);
            self.set_m_offset(ce, ZERO_FIELD);
            let mut jj = (m_spread - 1) as usize;
            while self.move_[jj] == self.sentinel() {
                jj -= 1;
            }
            self.set_n_min(ce, j as i32 + mm);
            self.set_n_max(ce, jj as i32 + mm);
            let mut q = ce;
            loop {
                let p = self.get_node(ROW_NODE_SIZE)?;
                self.set_link(q, p);
                self.set_knil(p, q);
                self.set_sorted(p, self.move_[j]);
                self.set_unsorted(p, NULL);
                j += 1;
                q = p;
                if j > jj {
                    break;
                }
            }
            self.set_link(q, ce);
            self.set_knil(ce, q);
            self.set_n_pos(ce, self.n_max(ce) + 1);
            self.set_n_rover(ce, ce);
            self.set_last_window_time(ce, 0);
        }
        Ok(())
    }

    // §366
    pub(crate) fn merge_edges(&mut self, h: Pointer) -> R {
        if self.link(h) != h {
            let ce = self.cur_edges;
            if self.m_min(h) < self.m_min(ce)
                || self.m_max(h) > self.m_max(ce)
                || self.n_min(h) < self.n_min(ce)
                || self.n_max(h) > self.n_max(ce)
            {
                self.edge_prep(
                    self.m_min(h) - ZERO_FIELD,
                    self.m_max(h) - ZERO_FIELD,
                    self.n_min(h) - ZERO_FIELD,
                    self.n_max(h) - ZERO_FIELD + 1,
                )?;
            }
            if self.m_offset(h) != self.m_offset(ce) {
                // §367
                let mut pp = self.link(h);
                let delta = 8 * (self.m_offset(ce) - self.m_offset(h));
                loop {
                    let mut qq = self.sorted(pp);
                    while qq != self.sentinel() {
                        let v = self.info(qq) + delta;
                        self.set_info(qq, v);
                        qq = self.link(qq);
                    }
                    qq = self.unsorted(pp);
                    while qq > VOID {
                        let v = self.info(qq) + delta;
                        self.set_info(qq, v);
                        qq = self.link(qq);
                    }
                    pp = self.link(pp);
                    if pp == h {
                        break;
                    }
                }
            }
            let mut n = self.n_min(ce);
            let mut p = self.link(ce);
            let mut pp = self.link(h);
            while n < self.n_min(h) {
                n += 1;
                p = self.link(p);
            }
            loop {
                // §368
                let mut qq = self.unsorted(pp);
                if qq > VOID {
                    if self.unsorted(p) <= VOID {
                        self.set_unsorted(p, qq);
                    } else {
                        while self.link(qq) > VOID {
                            qq = self.link(qq);
                        }
                        self.set_link(qq, self.unsorted(p));
                        self.set_unsorted(p, self.unsorted(pp));
                    }
                }
                self.set_unsorted(pp, NULL);
                qq = self.sorted(pp);
                if qq != self.sentinel() {
                    if self.unsorted(p) == VOID {
                        self.set_unsorted(p, NULL);
                    }
                    self.set_sorted(pp, self.sentinel());
                    let mut r = p + 1;
                    let mut q = self.link(r);
                    if q == self.sentinel() {
                        self.set_sorted(p, qq);
                    } else {
                        loop {
                            let k = self.info(qq);
                            while k > self.info(q) {
                                r = q;
                                q = self.link(r);
                            }
                            self.set_link(r, qq);
                            let rr = self.link(qq);
                            self.set_link(qq, q);
                            if rr == self.sentinel() {
                                break;
                            }
                            r = qq;
                            qq = rr;
                        }
                    }
                }
                pp = self.link(pp);
                p = self.link(p);
                if pp == h {
                    break;
                }
            }
        }
        Ok(())
    }

    // §369
    pub(crate) fn total_weight(&self, h: Pointer) -> i32 {
        let mut n = 0;
        let mut p = self.link(h);
        while p != h {
            let mut q = self.sorted(p);
            while q != self.sentinel() {
                let m = self.info(q);
                n -= ((m % 8) - ZERO_W) * (m / 8);
                q = self.link(q);
            }
            q = self.unsorted(p);
            while q > VOID {
                let m = self.info(q);
                n -= ((m % 8) - ZERO_W) * (m / 8);
                q = self.link(q);
            }
            p = self.link(p);
        }
        n
    }

    // §372
    pub(crate) fn begin_edge_tracing(&mut self) {
        self.print_diagnostic("Tracing edges", "", true);
        self.prints(" (weight ");
        self.print_int(self.cur_wt);
        self.print_char(b')');
        self.trace_x = -4096;
    }
    pub(crate) fn trace_a_corner(&mut self) {
        if self.file_offset > self.max_print_line - 13 {
            self.print_nl("");
        }
        self.print_char(b'(');
        self.print_int(self.trace_x);
        self.print_char(b',');
        self.print_int(self.trace_yy);
        self.print_char(b')');
        self.trace_y = self.trace_yy;
    }
    pub(crate) fn end_edge_tracing(&mut self) {
        if self.trace_x == -4096 {
            self.print_nl("(No new edges added.)");
        } else {
            self.trace_a_corner();
            self.print_char(b'.');
        }
        self.end_diagnostic(true);
    }

    // §373
    pub(crate) fn trace_new_edge(&mut self, r: Pointer, n: i32) {
        let d = self.info(r);
        let w = (d % 8) - ZERO_W;
        let m = (d / 8) - self.m_offset(self.cur_edges);
        let (n0, n1) = if w == self.cur_wt {
            (n + 1, n)
        } else {
            (n, n + 1)
        };
        if m != self.trace_x {
            if self.trace_x == -4096 {
                self.print_nl("");
                self.trace_yy = n0;
            } else if self.trace_yy != n0 {
                self.print_char(b'?');
            } else {
                self.trace_a_corner();
            }
            self.trace_x = m;
            self.trace_a_corner();
        } else {
            if n0 != self.trace_yy {
                self.print_char(b'!');
            }
            if (n0 < n1 && self.trace_y > self.trace_yy)
                || (n0 > n1 && self.trace_y < self.trace_yy)
            {
                self.trace_a_corner();
            }
        }
        self.trace_yy = n1;
    }

    // §374
    pub(crate) fn line_edges(&mut self, x0: Scaled, y0: Scaled, x1: Scaled, y1: Scaled) -> R {
        let mut n0 = round_unscaled(y0);
        let n1 = round_unscaled(y1);
        if n0 != n1 {
            let m0 = round_unscaled(x0);
            let m1 = round_unscaled(x1);
            let delx = x1 - x0;
            let dely = y1 - y0;
            let yt = n0 * UNITY - HALF_UNIT;
            let mut y0 = y0 - yt;
            let mut y1 = y1 - yt;
            let mut p;
            let mut n;
            if n0 < n1 {
                // §375
                let base = 8 * self.m_offset(self.cur_edges) + MIN_HALFWORD + ZERO_W - self.cur_wt;
                if m0 <= m1 {
                    self.edge_prep(m0, m1, n0, n1)?;
                } else {
                    self.edge_prep(m1, m0, n0, n1)?;
                }
                (n, p) = self.move_to_row(n0);
                y0 = UNITY - y0;
                loop {
                    let r = self.get_avail()?;
                    self.set_link(r, self.unsorted(p));
                    self.set_unsorted(p, r);
                    let f = self.make_fraction(y0, dely);
                    let mut tx = self.take_fraction(delx, f);
                    if ab_vs_cd(delx, y0, dely, tx) < 0 {
                        tx -= 1;
                    }
                    self.set_info(r, 8 * round_unscaled(x0 + tx) + base);
                    y1 -= UNITY;
                    if self.int(TRACING_EDGES) > 0 {
                        self.trace_new_edge(r, n);
                    }
                    if y1 < UNITY {
                        break;
                    }
                    p = self.link(p);
                    y0 += UNITY;
                    n += 1;
                }
            } else {
                // §376
                let base = 8 * self.m_offset(self.cur_edges) + MIN_HALFWORD + ZERO_W + self.cur_wt;
                if m0 <= m1 {
                    self.edge_prep(m0, m1, n1, n0)?;
                } else {
                    self.edge_prep(m1, m0, n1, n0)?;
                }
                n0 -= 1;
                (n, p) = self.move_to_row(n0);
                loop {
                    let r = self.get_avail()?;
                    self.set_link(r, self.unsorted(p));
                    self.set_unsorted(p, r);
                    let f = self.make_fraction(y0, dely);
                    let mut tx = self.take_fraction(delx, f);
                    if ab_vs_cd(delx, y0, dely, tx) < 0 {
                        tx += 1;
                    }
                    self.set_info(r, 8 * round_unscaled(x0 - tx) + base);
                    y1 += UNITY;
                    if self.int(TRACING_EDGES) > 0 {
                        self.trace_new_edge(r, n);
                    }
                    if y1 >= 0 {
                        break;
                    }
                    p = self.knil(p);
                    y0 += UNITY;
                    n -= 1;
                }
            }
            self.set_n_rover(self.cur_edges, p);
            self.set_n_pos(self.cur_edges, n + ZERO_FIELD);
        }
        Ok(())
    }

    // §377: (n, p) at row n0
    pub(crate) fn move_to_row(&mut self, n0: i32) -> (i32, Pointer) {
        let mut n = self.n_pos(self.cur_edges) - ZERO_FIELD;
        let mut p = self.n_rover(self.cur_edges);
        if n != n0 {
            if n < n0 {
                loop {
                    n += 1;
                    p = self.link(p);
                    if n == n0 {
                        break;
                    }
                }
            } else {
                loop {
                    n -= 1;
                    p = self.knil(p);
                    if n == n0 {
                        break;
                    }
                }
            }
        }
        (n, p)
    }

    // §378
    pub(crate) fn move_to_edges(&mut self, mut m0: i32, mut n0: i32, m1: i32, n1: i32) -> R {
        let delta = n1 - n0;
        let dx;
        // §380
        let case = match self.octant {
            FIRST_OCTANT => {
                dx = 8;
                self.edge_prep(m0, m1, n0, n1)?;
                0
            }
            SECOND_OCTANT => {
                dx = 8;
                self.edge_prep(n0, n1, m0, m1)?;
                2
            }
            THIRD_OCTANT => {
                dx = -8;
                self.edge_prep(-n1, -n0, m0, m1)?;
                n0 = -n0;
                2
            }
            FOURTH_OCTANT => {
                dx = -8;
                self.edge_prep(-m1, -m0, n0, n1)?;
                m0 = -m0;
                0
            }
            FIFTH_OCTANT => {
                dx = -8;
                self.edge_prep(-m1, -m0, -n1, -n0)?;
                m0 = -m0;
                1
            }
            SIXTH_OCTANT => {
                dx = -8;
                self.edge_prep(-n1, -n0, -m1, -m0)?;
                n0 = -n0;
                3
            }
            SEVENTH_OCTANT => {
                dx = 8;
                self.edge_prep(n0, n1, -m1, -m0)?;
                3
            }
            _ => {
                dx = 8;
                self.edge_prep(m0, m1, -n1, -n0)?;
                1
            }
        };
        let (mut n, mut p);
        match case {
            0 => {
                // §381
                (n, p) = self.move_to_row(n0);
                if delta > 0 {
                    let mut k = 0;
                    let mut edge_and_weight =
                        8 * (m0 + self.m_offset(self.cur_edges)) + MIN_HALFWORD + ZERO_W
                            - self.cur_wt;
                    loop {
                        edge_and_weight += dx * self.move_[k as usize];
                        let r = self.fast_get_avail()?;
                        self.set_link(r, self.unsorted(p));
                        self.set_info(r, edge_and_weight);
                        if self.int(TRACING_EDGES) > 0 {
                            self.trace_new_edge(r, n);
                        }
                        self.set_unsorted(p, r);
                        p = self.link(p);
                        k += 1;
                        n += 1;
                        if k == delta {
                            break;
                        }
                    }
                }
            }
            1 => {
                // §382
                n0 = -n0 - 1;
                (n, p) = self.move_to_row(n0);
                if delta > 0 {
                    let mut k = 0;
                    let mut edge_and_weight = 8 * (m0 + self.m_offset(self.cur_edges))
                        + MIN_HALFWORD
                        + ZERO_W
                        + self.cur_wt;
                    loop {
                        edge_and_weight += dx * self.move_[k as usize];
                        let r = self.fast_get_avail()?;
                        self.set_link(r, self.unsorted(p));
                        self.set_info(r, edge_and_weight);
                        if self.int(TRACING_EDGES) > 0 {
                            self.trace_new_edge(r, n);
                        }
                        self.set_unsorted(p, r);
                        p = self.knil(p);
                        k += 1;
                        n -= 1;
                        if k == delta {
                            break;
                        }
                    }
                }
            }
            2 => {
                // §383
                let mut edge_and_weight =
                    8 * (n0 + self.m_offset(self.cur_edges)) + MIN_HALFWORD + ZERO_W - self.cur_wt;
                n0 = m0;
                let mut k = 0;
                (n, p) = self.move_to_row(n0);
                loop {
                    let mut j = self.move_[k as usize];
                    while j > 0 {
                        let r = self.fast_get_avail()?;
                        self.set_link(r, self.unsorted(p));
                        self.set_info(r, edge_and_weight);
                        if self.int(TRACING_EDGES) > 0 {
                            self.trace_new_edge(r, n);
                        }
                        self.set_unsorted(p, r);
                        p = self.link(p);
                        j -= 1;
                        n += 1;
                    }
                    edge_and_weight += dx;
                    k += 1;
                    if k > delta {
                        break;
                    }
                }
            }
            _ => {
                // §384
                let mut edge_and_weight =
                    8 * (n0 + self.m_offset(self.cur_edges)) + MIN_HALFWORD + ZERO_W + self.cur_wt;
                n0 = -m0 - 1;
                let mut k = 0;
                (n, p) = self.move_to_row(n0);
                loop {
                    let mut j = self.move_[k as usize];
                    while j > 0 {
                        let r = self.fast_get_avail()?;
                        self.set_link(r, self.unsorted(p));
                        self.set_info(r, edge_and_weight);
                        if self.int(TRACING_EDGES) > 0 {
                            self.trace_new_edge(r, n);
                        }
                        self.set_unsorted(p, r);
                        p = self.knil(p);
                        j -= 1;
                        n -= 1;
                    }
                    edge_and_weight += dx;
                    k += 1;
                    if k > delta {
                        break;
                    }
                }
            }
        }
        self.set_n_pos(self.cur_edges, n + ZERO_FIELD);
        self.set_n_rover(self.cur_edges, p);
        Ok(())
    }

    // §385
    pub(crate) fn toss_edges(&mut self, h: Pointer) {
        let mut q = self.link(h);
        while q != h {
            self.flush_list(self.sorted(q));
            if self.unsorted(q) > VOID {
                self.flush_list(self.unsorted(q));
            }
            let p = q;
            q = self.link(q);
            self.free_node(p, ROW_NODE_SIZE);
        }
        self.free_node(h, EDGE_HEADER_SIZE);
    }
}
