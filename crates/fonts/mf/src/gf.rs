//! Parts 46 and 47 of `mf.web`: the generic font file, shipping
//! characters out (the output buffer kept as `mf.web` keeps it: its size
//! decides the trailing 223s).

use crate::arith::round_unscaled;
use crate::{Mf, R, StrNumber, consts::*};

const THREE_BYTES: i32 = 0o100000000;

impl Mf {
    fn write_gf(&mut self, a: i32, b: i32) {
        let s = self.gf_buf[a as usize..=b as usize].to_vec();
        self.gf_file.extend_from_slice(&s);
    }
    // §1155
    pub(crate) fn gf_out(&mut self, b: i32) -> R {
        self.gf_buf[self.gf_ptr as usize] = b as u8;
        self.gf_ptr += 1;
        if self.gf_ptr == self.gf_limit {
            self.gf_swap()?;
        }
        Ok(())
    }
    // §1155
    fn gf_swap(&mut self) -> R {
        if self.gf_ptr > 0x7FFF_FFFF - self.gf_offset {
            self.gf_prev_ptr = 0;
            return self.fatal_error("gf length exceeds \"7FFFFFFF");
        }
        if self.gf_limit == self.gf_buf_size {
            self.write_gf(0, self.half_buf - 1);
            self.gf_limit = self.half_buf;
            self.gf_offset += self.gf_buf_size;
            self.gf_ptr = 0;
        } else {
            self.write_gf(self.half_buf, self.gf_buf_size - 1);
            self.gf_limit = self.gf_buf_size;
        }
        Ok(())
    }
    // §1157
    fn gf_four(&mut self, mut x: i32) -> R {
        if x >= 0 {
            self.gf_out(x / THREE_BYTES)?;
        } else {
            x = x.wrapping_add(0o10000000000);
            x = x.wrapping_add(0o10000000000);
            self.gf_out((x / THREE_BYTES) + 128)?;
        }
        x %= THREE_BYTES;
        self.gf_out(x / UNITY)?;
        x %= UNITY;
        self.gf_out(x / 0o400)?;
        self.gf_out(x % 0o400)
    }
    // §1158
    fn gf_two(&mut self, x: i32) -> R {
        self.gf_out(x / 0o400)?;
        self.gf_out(x % 0o400)
    }
    fn gf_three(&mut self, x: i32) -> R {
        self.gf_out(x / UNITY)?;
        self.gf_out((x % UNITY) / 0o400)?;
        self.gf_out(x % 0o400)
    }
    // §1159
    fn gf_paint(&mut self, d: i32) -> R {
        if d < 64 {
            self.gf_out(PAINT_0 + d)
        } else if d < 256 {
            self.gf_out(PAINT1)?;
            self.gf_out(d)
        } else {
            self.gf_out(PAINT1 + 1)?;
            self.gf_two(d)
        }
    }
    // §1160
    fn gf_string(&mut self, s: StrNumber, t: StrNumber) -> R {
        if s != 0 {
            let mut l = self.length(s);
            if t != 0 {
                l += self.length(t);
            }
            if l <= 255 {
                self.gf_out(XXX1)?;
                self.gf_out(l)?;
            } else {
                self.gf_out(XXX3)?;
                self.gf_three(l)?;
            }
            for k in self.str_start[s as usize]..self.str_start[(s + 1) as usize] {
                self.gf_out(i32::from(self.str_pool[k as usize]))?;
            }
        }
        if t != 0 {
            for k in self.str_start[t as usize]..self.str_start[(t + 1) as usize] {
                self.gf_out(i32::from(self.str_pool[k as usize]))?;
            }
        }
        Ok(())
    }
    // §1161
    fn gf_boc(&mut self, min_m: i32, max_m: i32, min_n: i32, max_n: i32) -> R {
        if min_m < self.gf_min_m {
            self.gf_min_m = min_m;
        }
        if max_n > self.gf_max_n {
            self.gf_max_n = max_n;
        }
        let ob = |x: i32| (0..256).contains(&x);
        if self.boc_p == -1
            && ob(self.boc_c)
            && ob(max_m - min_m)
            && ob(max_m)
            && ob(max_n - min_n)
            && ob(max_n)
        {
            self.gf_out(BOC1)?;
            self.gf_out(self.boc_c)?;
            self.gf_out(max_m - min_m)?;
            self.gf_out(max_m)?;
            self.gf_out(max_n - min_n)?;
            return self.gf_out(max_n);
        }
        self.gf_out(BOC)?;
        self.gf_four(self.boc_c)?;
        self.gf_four(self.boc_p)?;
        self.gf_four(min_m)?;
        self.gf_four(max_m)?;
        self.gf_four(min_n)?;
        self.gf_four(max_n)
    }
    // §1162
    pub(crate) fn check_gf(&mut self) -> R {
        if self.output_file_name == 0 {
            self.init_gf()?;
        }
        Ok(())
    }
    // §1163
    fn init_gf(&mut self) -> R {
        self.gf_min_m = 4096;
        self.gf_max_m = -4096;
        self.gf_min_n = 4096;
        self.gf_max_n = -4096;
        for k in 0..256 {
            self.char_ptr[k] = -1;
        }
        // §1164
        if self.int(HPPP) <= 0 {
            self.gf_ext = self.pstr(".gf");
        } else {
            self.old_setting = self.selector;
            self.selector = NEW_STRING;
            self.print_char(b'.');
            let r = self.make_scaled(self.int(HPPP), 59_429_463);
            self.print_int(r);
            self.prints("gf");
            self.gf_ext = self.make_string()?;
            self.selector = self.old_setting;
        }
        self.set_output_file_name()?;
        self.gf_out(PRE)?;
        self.gf_out(GF_ID_BYTE)?;
        self.old_setting = self.selector;
        self.selector = NEW_STRING;
        self.prints(" METAFONT output ");
        self.print_int(round_unscaled(self.int(YEAR)));
        self.print_char(b'.');
        self.print_dd(round_unscaled(self.int(MONTH)));
        self.print_char(b'.');
        self.print_dd(round_unscaled(self.int(DAY)));
        self.print_char(b':');
        let t = round_unscaled(self.int(TIME));
        self.print_dd(t / 60);
        self.print_dd(t % 60);
        self.selector = self.old_setting;
        self.gf_out(self.cur_length())?;
        let s = self.make_string()?;
        self.gf_string(0, s)?;
        self.str_ptr -= 1;
        self.pool_ptr = self.str_start[self.str_ptr as usize];
        self.gf_prev_ptr = self.gf_offset + self.gf_ptr;
        Ok(())
    }

    // §1165
    pub(crate) fn ship_out(&mut self, c: i32) -> R {
        self.check_gf()?;
        let f = round_unscaled(self.int(CHAR_EXT));
        let x_off = round_unscaled(self.int(X_OFFSET));
        let y_off = round_unscaled(self.int(Y_OFFSET));
        if self.term_offset > self.max_print_line - 9 {
            self.print_ln();
        } else if self.term_offset > 0 || self.file_offset > 0 {
            self.print_char(b' ');
        }
        self.print_char(b'[');
        self.print_int(c);
        if f != 0 {
            self.print_char(b'.');
            self.print_int(f);
        }
        self.update_terminal();
        self.boc_c = 256 * f + c;
        self.boc_p = self.char_ptr[c as usize];
        self.char_ptr[c as usize] = self.gf_prev_ptr;
        if self.int(PROOFING) > 0 {
            // §1166
            if x_off != 0 {
                self.gf_string(self.pstr("xoffset"), 0)?;
                self.gf_out(YYY)?;
                self.gf_four(x_off * UNITY)?;
            }
            if y_off != 0 {
                self.gf_string(self.pstr("yoffset"), 0)?;
                self.gf_out(YYY)?;
                self.gf_four(y_off * UNITY)?;
            }
        }
        // §1167
        let ce = self.cur_edges;
        let mut prev_n = 4096;
        let mut p = self.knil(ce);
        let mut n = self.n_max(ce) - ZERO_FIELD;
        let mut cur_min_m = 0;
        while p != ce {
            // §1169
            if self.unsorted(p) > VOID {
                self.sort_edges(p);
            }
            let mut q = self.sorted(p);
            let mut w = 0;
            let mut prev_m = -FRACTION_ONE;
            let mut ww = 0;
            let mut prev_w = 0;
            let mut m = prev_m;
            loop {
                let mm;
                if q == self.sentinel() {
                    mm = FRACTION_ONE;
                } else {
                    let d = self.info(q);
                    mm = d / 8;
                    ww = ww + (d % 8) - ZERO_W;
                }
                if mm != m {
                    if prev_w <= 0 {
                        if w > 0 {
                            // §1170
                            if prev_m == -FRACTION_ONE {
                                // §1172
                                'done: {
                                    if prev_n == 4096 {
                                        self.gf_boc(
                                            self.m_min(ce) + x_off - ZERO_FIELD,
                                            self.m_max(ce) + x_off - ZERO_FIELD,
                                            self.n_min(ce) + y_off - ZERO_FIELD,
                                            n + y_off,
                                        )?;
                                        cur_min_m = self.m_min(ce) - ZERO_FIELD + self.m_offset(ce);
                                    } else if prev_n > n + 1 {
                                        // §1174
                                        let delta = prev_n - n - 1;
                                        if delta < 0o400 {
                                            self.gf_out(SKIP1)?;
                                            self.gf_out(delta)?;
                                        } else {
                                            self.gf_out(SKIP1 + 1)?;
                                            self.gf_two(delta)?;
                                        }
                                    } else {
                                        // §1173
                                        let delta = m - cur_min_m;
                                        if delta > MAX_NEW_ROW {
                                            self.gf_out(SKIP0)?;
                                        } else {
                                            self.gf_out(NEW_ROW_0 + delta)?;
                                            break 'done;
                                        }
                                    }
                                    self.gf_paint(m - cur_min_m)?;
                                }
                                prev_n = n;
                            } else {
                                self.gf_paint(m - prev_m)?;
                            }
                            prev_m = m;
                            prev_w = w;
                        }
                    } else if w <= 0 {
                        // §1171
                        self.gf_paint(m - prev_m)?;
                        prev_m = m;
                        prev_w = w;
                    }
                    m = mm;
                }
                w = ww;
                q = self.link(q);
                if mm == FRACTION_ONE {
                    break;
                }
            }
            if w != 0 {
                self.print_nl("(There's unbounded black in character shipped out!)");
            }
            if prev_m - self.m_offset(ce) + x_off > self.gf_max_m {
                self.gf_max_m = prev_m - self.m_offset(ce) + x_off;
            }
            p = self.knil(p);
            n -= 1;
        }
        if prev_n == 4096 {
            // §1168
            self.gf_boc(0, 0, 0, 0)?;
            if self.gf_max_m < 0 {
                self.gf_max_m = 0;
            }
            if self.gf_min_n > 0 {
                self.gf_min_n = 0;
            }
        } else if prev_n + y_off < self.gf_min_n {
            self.gf_min_n = prev_n + y_off;
        }
        self.gf_out(EOC)?;
        self.gf_prev_ptr = self.gf_offset + self.gf_ptr;
        self.total_chars += 1;
        self.print_char(b']');
        self.update_terminal();
        if self.int(TRACING_OUTPUT) > 0 {
            self.print_edges(" (just shipped out)", true, x_off, y_off);
        }
        Ok(())
    }

    // §1176
    pub(crate) fn do_special(&mut self) -> R {
        let m = self.cur_mod;
        self.get_x_next()?;
        self.scan_expression()?;
        if self.int(PROOFING) >= 0 {
            if self.cur_type != m {
                // §1178
                self.exp_err("Unsuitable expression")?;
                self.help(&["The expression shown above has the wrong type to be output."]);
                self.put_get_error()?;
            } else {
                self.check_gf()?;
                if m == STRING_TYPE {
                    self.gf_string(self.cur_exp, 0)?;
                } else {
                    self.gf_out(YYY)?;
                    self.gf_four(self.cur_exp)?;
                }
            }
        }
        self.flush_cur_exp(0)
    }

    // §1179
    pub(crate) fn send_title(&mut self) -> R {
        self.check_gf()?;
        self.gf_string(self.pstr("title "), self.cur_exp)
    }

    // §1182
    pub(crate) fn finish_gf(&mut self) -> R {
        self.gf_out(POST)?;
        self.gf_four(self.gf_prev_ptr)?;
        self.gf_prev_ptr = self.gf_offset + self.gf_ptr - 5;
        self.gf_four(self.int(DESIGN_SIZE) * 16)?;
        for k in 1..=4 {
            self.gf_out(self.header_byte[k])?;
        }
        self.gf_four(self.int(HPPP))?;
        self.gf_four(self.int(VPPP))?;
        self.gf_four(self.gf_min_m)?;
        self.gf_four(self.gf_max_m)?;
        self.gf_four(self.gf_min_n)?;
        self.gf_four(self.gf_max_n)?;
        for k in 0..256usize {
            if self.char_exists[k] {
                let mut x = self.gf_dx[k] / UNITY;
                if self.gf_dy[k] == 0 && x >= 0 && x < 256 && self.gf_dx[k] == x * UNITY {
                    self.gf_out(CHAR_LOC + 1)?;
                    self.gf_out(k as i32)?;
                    self.gf_out(x)?;
                } else {
                    self.gf_out(CHAR_LOC)?;
                    self.gf_out(k as i32)?;
                    self.gf_four(self.gf_dx[k])?;
                    self.gf_four(self.gf_dy[k])?;
                }
                x = self.value(self.tfm_width[k]);
                if x.abs() > self.max_tfm_dimen {
                    x = if x > 0 {
                        THREE_BYTES - 1
                    } else {
                        1 - THREE_BYTES
                    };
                } else {
                    x = self.make_scaled(x * 16, self.int(DESIGN_SIZE));
                }
                self.gf_four(x)?;
                self.gf_four(self.char_ptr[k])?;
            }
        }
        self.gf_out(POST_POST)?;
        self.gf_four(self.gf_prev_ptr)?;
        self.gf_out(GF_ID_BYTE)?;
        let mut k = 4 + ((self.gf_buf_size - self.gf_ptr) % 4);
        while k > 0 {
            self.gf_out(223)?;
            k -= 1;
        }
        // §1156
        if self.gf_limit == self.half_buf {
            self.write_gf(self.half_buf, self.gf_buf_size - 1);
        }
        if self.gf_ptr > 0x7FFF_FFFF - self.gf_offset {
            self.gf_prev_ptr = 0;
            return self.fatal_error("gf length exceeds \"7FFFFFFF");
        }
        if self.gf_ptr > 0 {
            self.write_gf(0, self.gf_ptr - 1);
        }
        self.print_nl("Output written on ");
        self.print_file_name(0, self.output_file_name, 0);
        self.prints(" (");
        self.print_int(self.total_chars);
        if self.total_chars != 1 {
            self.prints(" characters");
        } else {
            self.prints(" character");
        }
        self.prints(", ");
        self.print_int(self.gf_offset + self.gf_ptr);
        self.prints(" bytes).");
        let data = std::mem::take(&mut self.gf_file);
        let name = self.gf_file_name.clone();
        self.outputs.push((name, data));
        Ok(())
    }
}
