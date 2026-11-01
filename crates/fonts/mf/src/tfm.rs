//! Part 45 of `mf.web`: font metric data (with `mf.ch`'s `put_2_bytes`
//! and `put_4_bytes`).

use crate::arith::{halfp, round_unscaled};
use crate::{FourQuarters, Mf, Pointer, R, Scaled, consts::*};

const UNDEFINED_LABEL: i32 = LIG_TABLE_SIZE;

impl Mf {
    // §1098
    pub(crate) fn tfm_check(&mut self, m: i32) -> R<Scaled> {
        if self.int(m).abs() >= FRACTION_HALF {
            self.print_err("Enormous ");
            self.print(self.int_name[m as usize]);
            self.prints(" has been reduced");
            self.help(&["Font metric dimensions must be less than 2048pt."]);
            self.put_get_error()?;
            if self.int(m) > 0 {
                Ok(FRACTION_HALF - 1)
            } else {
                Ok(1 - FRACTION_HALF)
            }
        } else {
            Ok(self.int(m))
        }
    }

    // §1099
    pub(crate) fn store_width_info(&mut self, c: i32) -> R {
        if c < self.bc {
            self.bc = c;
        }
        if c > self.ec {
            self.ec = c;
        }
        let c = c as usize;
        self.char_exists[c] = true;
        self.gf_dx[c] = self.int(CHAR_DX);
        self.gf_dy[c] = self.int(CHAR_DY);
        self.tfm_width[c] = self.tfm_check(CHAR_WD)?;
        self.tfm_height[c] = self.tfm_check(CHAR_HT)?;
        self.tfm_depth[c] = self.tfm_check(CHAR_DP)?;
        self.tfm_ital_corr[c] = self.tfm_check(CHAR_IC)?;
        Ok(())
    }

    // §1103
    pub(crate) fn get_code(&mut self) -> R<i32> {
        self.get_x_next()?;
        self.scan_expression()?;
        if self.cur_type == KNOWN {
            let c = round_unscaled(self.cur_exp);
            if (0..256).contains(&c) {
                return Ok(c);
            }
        } else if self.cur_type == STRING_TYPE && self.length(self.cur_exp) == 1 {
            return Ok(i32::from(
                self.str_pool[self.str_start[self.cur_exp as usize] as usize],
            ));
        }
        self.exp_err("Invalid code has been replaced by 0")?;
        self.help(&[
            "I was looking for a number between 0 and 255, or for a",
            "string of length 1. Didn't find it; will use 0 instead.",
        ]);
        self.put_get_flush_error(0)?;
        Ok(0)
    }

    // §1104
    pub(crate) fn set_tag(&mut self, c: i32, t: i32, r: i32) -> R {
        let cu = c as usize;
        if cu < 256 && self.char_tag[cu] == NO_TAG {
            self.char_tag[cu] = t;
            self.char_remainder[cu] = r;
            if t == LIG_TAG {
                self.label_ptr += 1;
                self.label_loc[self.label_ptr as usize] = r;
                self.label_char[self.label_ptr as usize] = c;
            }
            Ok(())
        } else {
            // §1105
            self.print_err("Character ");
            if c > i32::from(b' ') && c < 127 {
                self.print(c);
            } else if c == 256 {
                self.prints("||");
            } else {
                self.prints("code ");
                self.print_int(c);
            }
            self.prints(" is already ");
            match self.char_tag[cu.min(255)] {
                LIG_TAG => self.prints("in a ligtable"),
                LIST_TAG => self.prints("in a charlist"),
                EXT_TAG => self.prints("extensible"),
                _ => {}
            }
            self.help(&[
                "It's not legal to label a character more than once.",
                "So I'll not change anything just now.",
            ]);
            self.put_get_error()
        }
    }

    #[inline]
    fn skip_byte(&self, k: i32) -> i32 {
        i32::from(self.lig_kern[k as usize].b0)
    }
    #[inline]
    fn set_skip_byte(&mut self, k: i32, v: i32) {
        self.lig_kern[k as usize].b0 = v as u8;
    }
    // §1111
    fn cancel_skips(&mut self, ll: i32) {
        self.ll = ll;
        loop {
            self.lll = self.skip_byte(self.ll);
            self.set_skip_byte(self.ll, STOP_FLAG);
            self.ll -= self.lll;
            if self.lll == 0 {
                break;
            }
        }
    }
    fn skip_error(&mut self, ll: i32) -> R {
        self.print_err("Too far to skip");
        self.help(&["At most 127 lig/kern steps can separate skipto1 from 1::."]);
        self.error()?;
        self.cancel_skips(ll);
        Ok(())
    }
    fn missing_extensible_punctuation(&mut self, s: &str) -> R {
        self.missing_err(s);
        self.help(&["I'm processing `extensible c: t,m,b,r'."]);
        self.back_error()
    }

    // §1106
    pub(crate) fn do_tfm_command(&mut self) -> R {
        match self.cur_mod {
            CHAR_LIST_CODE => {
                let mut c = self.get_code()?;
                while self.cur_cmd == COLON {
                    let cc = self.get_code()?;
                    self.set_tag(c, LIST_TAG, cc)?;
                    c = cc;
                }
            }
            LIG_TABLE_CODE => {
                // §1107
                self.lk_started = false;
                'done: loop {
                    // continue:
                    self.get_x_next()?;
                    if self.cur_cmd == SKIP_TO && self.lk_started {
                        // §1111
                        let c = self.get_code()? as usize;
                        if self.nl - self.skip_table[c] > 128 {
                            self.skip_error(self.skip_table[c])?;
                            self.skip_table[c] = UNDEFINED_LABEL;
                        }
                        let nl = self.nl;
                        if self.skip_table[c] == UNDEFINED_LABEL {
                            self.set_skip_byte(nl - 1, 0);
                        } else {
                            self.set_skip_byte(nl - 1, nl - self.skip_table[c] - 1);
                        }
                        self.skip_table[c] = nl - 1;
                        break 'done;
                    }
                    let c;
                    if self.cur_cmd == BCHAR_LABEL {
                        c = 256;
                        self.cur_cmd = COLON;
                    } else {
                        self.back_input()?;
                        c = self.get_code()?;
                    }
                    if self.cur_cmd == COLON || self.cur_cmd == DOUBLE_COLON {
                        // §1112
                        if self.cur_cmd == COLON {
                            if c == 256 {
                                self.bch_label = self.nl;
                            } else {
                                self.set_tag(c, LIG_TAG, self.nl)?;
                            }
                        } else if self.skip_table[c as usize] < UNDEFINED_LABEL {
                            self.ll = self.skip_table[c as usize];
                            self.skip_table[c as usize] = UNDEFINED_LABEL;
                            loop {
                                self.lll = self.skip_byte(self.ll);
                                if self.nl - self.ll > 128 {
                                    self.skip_error(self.ll)?;
                                    continue 'done;
                                }
                                let v = self.nl - self.ll - 1;
                                self.set_skip_byte(self.ll, v);
                                self.ll -= self.lll;
                                if self.lll == 0 {
                                    break;
                                }
                            }
                        }
                        continue 'done;
                    }
                    let nl = self.nl as usize;
                    if self.cur_cmd == LIG_KERN_TOKEN {
                        // §1113
                        self.lig_kern[nl].b1 = c as u8;
                        self.lig_kern[nl].b0 = 0;
                        if self.cur_mod < 128 {
                            self.lig_kern[nl].b2 = self.cur_mod as u8;
                            let r = self.get_code()?;
                            self.lig_kern[nl].b3 = r as u8;
                        } else {
                            self.get_x_next()?;
                            self.scan_expression()?;
                            if self.cur_type != KNOWN {
                                self.exp_err("Improper kern")?;
                                self.help(&[
                                    "The amount of kern should be a known numeric value.",
                                    "I'm zeroing this one. Proceed, with fingers crossed.",
                                ]);
                                self.put_get_flush_error(0)?;
                            }
                            self.kern[self.nk as usize] = self.cur_exp;
                            let mut k = 0;
                            while self.kern[k as usize] != self.cur_exp {
                                k += 1;
                            }
                            if k == self.nk {
                                if self.nk == MAX_KERNS {
                                    return self.overflow("kern", MAX_KERNS);
                                }
                                self.nk += 1;
                            }
                            self.lig_kern[nl].b2 = (KERN_FLAG + k / 256) as u8;
                            self.lig_kern[nl].b3 = (k % 256) as u8;
                        }
                        self.lk_started = true;
                    } else {
                        self.print_err("Illegal ligtable step");
                        self.help(&["I was looking for `=:' or `kern' here."]);
                        self.back_error()?;
                        self.lig_kern[nl].b1 = 0;
                        self.lig_kern[nl].b2 = 0;
                        self.lig_kern[nl].b3 = 0;
                        self.lig_kern[nl].b0 = (STOP_FLAG + 1) as u8;
                    }
                    if self.nl == LIG_TABLE_SIZE {
                        return self.overflow("ligtable size", LIG_TABLE_SIZE);
                    }
                    self.nl += 1;
                    if self.cur_cmd == COMMA {
                        continue 'done;
                    }
                    if self.skip_byte(self.nl - 1) < STOP_FLAG {
                        self.set_skip_byte(self.nl - 1, STOP_FLAG);
                    }
                    break 'done;
                }
            }
            EXTENSIBLE_CODE => {
                // §1113
                if self.ne == 256 {
                    return self.overflow("extensible", 256);
                }
                let c = self.get_code()?;
                self.set_tag(c, EXT_TAG, self.ne)?;
                if self.cur_cmd != COLON {
                    self.missing_extensible_punctuation(":")?;
                }
                let ne = self.ne as usize;
                self.exten[ne].b0 = self.get_code()? as u8;
                if self.cur_cmd != COMMA {
                    self.missing_extensible_punctuation(",")?;
                }
                self.exten[ne].b1 = self.get_code()? as u8;
                if self.cur_cmd != COMMA {
                    self.missing_extensible_punctuation(",")?;
                }
                self.exten[ne].b2 = self.get_code()? as u8;
                if self.cur_cmd != COMMA {
                    self.missing_extensible_punctuation(",")?;
                }
                self.exten[ne].b3 = self.get_code()? as u8;
                self.ne += 1;
            }
            _ => {
                // HEADER_BYTE_CODE, FONT_DIMEN_CODE
                let c = self.cur_mod;
                self.get_x_next()?;
                self.scan_expression()?;
                if self.cur_type != KNOWN || self.cur_exp < HALF_UNIT {
                    self.exp_err("Improper location")?;
                    self.help(&[
                        "I was looking for a known, positive number.",
                        "For safety's sake I'll ignore the present command.",
                    ]);
                    self.put_get_error()?;
                } else {
                    let mut j = round_unscaled(self.cur_exp);
                    if self.cur_cmd != COLON {
                        self.missing_err(":");
                        self.help(&["A colon should follow a headerbyte or fontdimen location."]);
                        self.back_error()?;
                    }
                    if c == HEADER_BYTE_CODE {
                        // §1114
                        loop {
                            if j > HEADER_SIZE {
                                return self.overflow("headerbyte", HEADER_SIZE);
                            }
                            self.header_byte[j as usize] = self.get_code()?;
                            j += 1;
                            if self.cur_cmd != COMMA {
                                break;
                            }
                        }
                    } else {
                        // §1115
                        loop {
                            if j > MAX_FONT_DIMEN {
                                return self.overflow("fontdimen", MAX_FONT_DIMEN);
                            }
                            while j > self.np {
                                self.np += 1;
                                self.param[self.np as usize] = 0;
                            }
                            self.get_x_next()?;
                            self.scan_expression()?;
                            if self.cur_type != KNOWN {
                                self.exp_err("Improper font parameter")?;
                                self.help(&[
                                    "I'm zeroing this one. Proceed, with fingers crossed.",
                                ]);
                                self.put_get_flush_error(0)?;
                            }
                            self.param[j as usize] = self.cur_exp;
                            j += 1;
                            if self.cur_cmd != COMMA {
                                break;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // §1117
    pub(crate) fn sort_in(&mut self, v: Scaled) -> R<Pointer> {
        let mut p = self.temp_head();
        let mut q;
        loop {
            q = self.link(p);
            if v <= self.value(q) {
                break;
            }
            p = q;
        }
        if v < self.value(q) {
            let r = self.get_node(VALUE_NODE_SIZE)?;
            self.set_value(r, v);
            self.set_link(r, q);
            self.set_link(p, r);
        }
        Ok(self.link(p))
    }

    // §1118
    pub(crate) fn min_cover(&mut self, d: Scaled) -> i32 {
        let mut m = 0;
        let mut p = self.link(self.temp_head());
        self.perturbation = EL_GORDO;
        while p != INF_VAL {
            m += 1;
            let l = self.value(p);
            loop {
                p = self.link(p);
                if self.value(p) > l + d {
                    break;
                }
            }
            if self.value(p) - l < self.perturbation {
                self.perturbation = self.value(p) - l;
            }
        }
        m
    }

    // §1120
    pub(crate) fn threshold_fn(&mut self, m: i32) -> Scaled {
        self.excess = self.min_cover(0) - m;
        if self.excess <= 0 {
            0
        } else {
            let mut d;
            loop {
                d = self.perturbation;
                if self.min_cover(d + d) <= m {
                    break;
                }
            }
            while self.min_cover(d) > m {
                d = self.perturbation;
            }
            d
        }
    }

    // §1121
    pub(crate) fn skimp(&mut self, _m: i32) -> i32 {
        let mut d = self.threshold_fn(_m);
        self.perturbation = 0;
        let mut q = self.temp_head();
        let mut m = 0;
        let mut p = self.link(self.temp_head());
        while p != INF_VAL {
            m += 1;
            let l = self.value(p);
            self.set_info(p, m);
            if self.value(self.link(p)) <= l + d {
                // §1122
                loop {
                    p = self.link(p);
                    self.set_info(p, m);
                    self.excess -= 1;
                    if self.excess == 0 {
                        d = 0;
                    }
                    if self.value(self.link(p)) > l + d {
                        break;
                    }
                }
                let v = l + halfp(self.value(p) - l);
                if self.value(p) - v > self.perturbation {
                    self.perturbation = self.value(p) - v;
                }
                let mut r = q;
                loop {
                    r = self.link(r);
                    self.set_value(r, v);
                    if r == p {
                        break;
                    }
                }
                self.set_link(q, p);
            }
            q = p;
            p = self.link(p);
        }
        m
    }

    // §1123
    pub(crate) fn tfm_warning(&mut self, m: i32) {
        self.print_nl("(some ");
        self.print(self.int_name[m as usize]);
        self.prints(" values had to be adjusted by as much as ");
        self.print_scaled(self.perturbation);
        self.prints("pt)");
    }

    // §1126
    pub(crate) fn massage_hdi(&mut self) -> R {
        let th = self.temp_head();
        for (which, max, code) in [(2usize, 15, CHAR_HT), (3, 15, CHAR_DP), (4, 63, CHAR_IC)] {
            self.set_link(th, INF_VAL);
            for k in self.bc..=self.ec {
                let k = k as usize;
                if self.char_exists[k] {
                    let v = match which {
                        2 => self.tfm_height[k],
                        3 => self.tfm_depth[k],
                        _ => self.tfm_ital_corr[k],
                    };
                    let nv = if v == 0 { ZERO_VAL } else { self.sort_in(v)? };
                    match which {
                        2 => self.tfm_height[k] = nv,
                        3 => self.tfm_depth[k] = nv,
                        _ => self.tfm_ital_corr[k] = nv,
                    }
                }
            }
            let n = self.skimp(max) + 1;
            match which {
                2 => self.nh = n,
                3 => self.nd = n,
                _ => self.ni = n,
            }
            self.dimen_head[which] = self.link(th);
            if self.perturbation >= 0o10000 {
                self.tfm_warning(code);
            }
        }
        Ok(())
    }

    // §1128
    pub(crate) fn fix_design_size(&mut self) {
        let mut d = self.int(DESIGN_SIZE);
        if d < UNITY || d >= FRACTION_HALF {
            if d != 0 {
                self.print_nl("(illegal design size has been changed to 128pt)");
            }
            d = 0o40000000;
            self.internal[DESIGN_SIZE as usize] = d;
        }
        if self.header_byte[5] < 0
            && self.header_byte[6] < 0
            && self.header_byte[7] < 0
            && self.header_byte[8] < 0
        {
            self.header_byte[5] = d / 0o4000000;
            self.header_byte[6] = (d / 4096) % 256;
            self.header_byte[7] = (d / 16) % 256;
            self.header_byte[8] = (d % 16) * 16;
        }
        self.max_tfm_dimen = 16 * self.int(DESIGN_SIZE) - 1 - self.int(DESIGN_SIZE) / 0o10000000;
        if self.max_tfm_dimen >= FRACTION_HALF {
            self.max_tfm_dimen = FRACTION_HALF - 1;
        }
    }

    // §1129
    pub(crate) fn dimen_out(&mut self, mut x: Scaled) -> i32 {
        if x.abs() > self.max_tfm_dimen {
            self.tfm_changed += 1;
            x = if x > 0 {
                self.max_tfm_dimen
            } else {
                -self.max_tfm_dimen
            };
        }
        self.make_scaled(x * 16, self.int(DESIGN_SIZE))
    }

    // §1131
    pub(crate) fn fix_check_sum(&mut self) {
        if self.header_byte[1] < 0
            && self.header_byte[2] < 0
            && self.header_byte[3] < 0
            && self.header_byte[4] < 0
        {
            // §1132
            let (mut b1, mut b2, mut b3, mut b4) = (self.bc, self.ec, self.bc, self.ec);
            self.tfm_changed = 0;
            for k in self.bc..=self.ec {
                if self.char_exists[k as usize] {
                    let w = self.value(self.tfm_width[k as usize]);
                    let x = self.dimen_out(w) + (k + 4) * 0o20000000;
                    b1 = (b1 + b1 + x) % 255;
                    b2 = (b2 + b2 + x) % 253;
                    b3 = (b3 + b3 + x) % 251;
                    b4 = (b4 + b4 + x) % 247;
                }
            }
            self.header_byte[1] = b1;
            self.header_byte[2] = b2;
            self.header_byte[3] = b3;
            self.header_byte[4] = b4;
            return;
        }
        for k in 1..=4 {
            if self.header_byte[k] < 0 {
                self.header_byte[k] = 0;
            }
        }
    }

    fn tfm_out(&mut self, b: i32) {
        self.tfm_file.push(b as u8);
    }
    fn tfm_two(&mut self, b: i32) {
        self.tfm_file.push(((b >> 8) & 255) as u8);
        self.tfm_file.push((b & 255) as u8);
    }
    fn tfm_four(&mut self, b: i32) {
        self.tfm_file.extend_from_slice(&b.to_be_bytes());
    }
    fn tfm_qqqq(&mut self, x: FourQuarters) {
        self.tfm_file.extend_from_slice(&[x.b0, x.b1, x.b2, x.b3]);
    }

    // §1134
    pub(crate) fn finish_tfm(&mut self) -> R {
        if self.job_name == 0 {
            self.open_log_file()?;
        }
        self.pack_job_name(self.pstr(".tfm"));
        self.tfm_file = Vec::new();
        let tfm_name = self.name_of_file.clone();
        self.metric_file_name = self.make_name_string()?;
        // §1135
        let mut k = HEADER_SIZE;
        while self.header_byte[k as usize] < 0 {
            k -= 1;
        }
        let lh = (k + 3) / 4;
        if self.bc > self.ec {
            self.bc = 1;
        }
        // §1137
        let mut lk_offset;
        self.bchar = round_unscaled(self.int(BOUNDARY_CHAR));
        if self.bchar < 0 || self.bchar > 255 {
            self.bchar = -1;
            self.lk_started = false;
            lk_offset = 0;
        } else {
            self.lk_started = true;
            lk_offset = 1;
        }
        // §1138
        let mut k = self.label_ptr;
        if self.label_loc[k as usize] + lk_offset > 255 {
            lk_offset = 0;
            self.lk_started = false;
            loop {
                let lc = self.label_char[k as usize] as usize;
                self.char_remainder[lc] = lk_offset;
                while self.label_loc[(k - 1) as usize] == self.label_loc[k as usize] {
                    k -= 1;
                    let lc = self.label_char[k as usize] as usize;
                    self.char_remainder[lc] = lk_offset;
                }
                lk_offset += 1;
                k -= 1;
                if lk_offset + self.label_loc[k as usize] < 256 {
                    break;
                }
            }
        }
        if lk_offset > 0 {
            while k > 0 {
                let lc = self.label_char[k as usize] as usize;
                self.char_remainder[lc] += lk_offset;
                k -= 1;
            }
        }
        if self.bch_label < UNDEFINED_LABEL {
            let nl = self.nl as usize;
            self.lig_kern[nl].b0 = 255;
            self.lig_kern[nl].b1 = 0;
            self.lig_kern[nl].b2 = ((self.bch_label + lk_offset) / 256) as u8;
            self.lig_kern[nl].b3 = ((self.bch_label + lk_offset) % 256) as u8;
            self.nl += 1;
        }
        let total = 6
            + lh
            + (self.ec - self.bc + 1)
            + self.nw
            + self.nh
            + self.nd
            + self.ni
            + self.nl
            + lk_offset
            + self.nk
            + self.ne
            + self.np;
        self.tfm_two(total);
        self.tfm_two(lh);
        self.tfm_two(self.bc);
        self.tfm_two(self.ec);
        self.tfm_two(self.nw);
        self.tfm_two(self.nh);
        self.tfm_two(self.nd);
        self.tfm_two(self.ni);
        self.tfm_two(self.nl + lk_offset);
        self.tfm_two(self.nk);
        self.tfm_two(self.ne);
        self.tfm_two(self.np);
        for k in 1..=4 * lh {
            if self.header_byte[k as usize] < 0 {
                self.header_byte[k as usize] = 0;
            }
            self.tfm_out(self.header_byte[k as usize]);
        }
        // §1136
        for k in self.bc..=self.ec {
            let k = k as usize;
            if !self.char_exists[k] {
                self.tfm_four(0);
            } else {
                self.tfm_out(self.info(self.tfm_width[k]));
                self.tfm_out(self.info(self.tfm_height[k]) * 16 + self.info(self.tfm_depth[k]));
                self.tfm_out(self.info(self.tfm_ital_corr[k]) * 4 + self.char_tag[k]);
                self.tfm_out(self.char_remainder[k]);
            }
        }
        self.tfm_changed = 0;
        for k in 1..=4 {
            self.tfm_four(0);
            let mut p = self.dimen_head[k];
            while p != INF_VAL {
                let d = self.dimen_out(self.value(p));
                self.tfm_four(d);
                p = self.link(p);
            }
        }
        // §1139
        for k in 0..256 {
            if self.skip_table[k] < UNDEFINED_LABEL {
                self.print_nl("(local label ");
                self.print_int(k as i32);
                self.prints(":: was missing)");
                self.cancel_skips(self.skip_table[k]);
            }
        }
        if self.lk_started {
            self.tfm_out(255);
            self.tfm_out(self.bchar);
            self.tfm_two(0);
        } else {
            for _ in 1..=lk_offset {
                self.ll = self.label_loc[self.label_ptr as usize];
                if self.bchar < 0 {
                    self.tfm_out(254);
                    self.tfm_out(0);
                } else {
                    self.tfm_out(255);
                    self.tfm_out(self.bchar);
                }
                self.tfm_two(self.ll + lk_offset);
                loop {
                    self.label_ptr -= 1;
                    if self.label_loc[self.label_ptr as usize] < self.ll {
                        break;
                    }
                }
            }
        }
        for k in 0..self.nl as usize {
            self.tfm_qqqq(self.lig_kern[k]);
        }
        for k in 0..self.nk as usize {
            let d = self.dimen_out(self.kern[k]);
            self.tfm_four(d);
        }
        // §1140
        for k in 0..self.ne as usize {
            self.tfm_qqqq(self.exten[k]);
        }
        for k in 1..=self.np as usize {
            if k == 1 {
                if self.param[1].abs() < FRACTION_HALF {
                    self.tfm_four(self.param[1] * 16);
                } else {
                    self.tfm_changed += 1;
                    if self.param[1] > 0 {
                        self.tfm_four(EL_GORDO);
                    } else {
                        self.tfm_four(-EL_GORDO);
                    }
                }
            } else {
                let d = self.dimen_out(self.param[k]);
                self.tfm_four(d);
            }
        }
        if self.tfm_changed > 0 {
            if self.tfm_changed == 1 {
                self.print_nl("(a font metric dimension");
            } else {
                self.print_nl("(");
                self.print_int(self.tfm_changed);
                self.prints(" font metric dimensions");
            }
            self.prints(" had to be decreased)");
        }
        if self.int(TRACING_STATS) > 0 {
            // §1141
            self.wlog_ln(b" ");
            if self.bch_label < UNDEFINED_LABEL {
                self.nl -= 1;
            }
            let s = format!(
                "(You used {}w,{}h,{}d,{}i,{}l,{}k,{}e,{}p metric file positions",
                self.nw, self.nh, self.nd, self.ni, self.nl, self.nk, self.ne, self.np
            );
            self.wlog_ln(s.as_bytes());
            let s = format!(
                "  out of 256w,16h,16d,64i,{LIG_TABLE_SIZE}l,{MAX_KERNS}k,256e,{MAX_FONT_DIMEN}p)"
            );
            self.wlog_ln(s.as_bytes());
        }
        self.print_nl("Font metrics written on ");
        self.print_file_name(0, self.metric_file_name, 0);
        self.print_char(b'.');
        let data = std::mem::take(&mut self.tfm_file);
        self.outputs.push((tfm_name, data));
        Ok(())
    }
}
