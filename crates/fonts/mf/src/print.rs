//! Parts 3 to 6 of `mf.web`: input lines, strings, printing, errors.

use crate::{AlphaFile, Mf, R, Stop, StrNumber, consts::*};

/// The banner (§2).
pub const BANNER: &str = "This is METAFONT, Version 2.71828182";

impl Mf {
    // §30 (web2c's `input_line`, lib/texmfmp.c): the next line of `f`
    // into `buffer[first..last)`, trailing spaces removed; false at the
    // end of the file.
    pub(crate) fn input_ln_from(&mut self, f: &mut AlphaFile) -> R<bool> {
        self.last = self.first;
        let mut i: i32 = -1;
        loop {
            if self.last >= self.buf_size {
                break;
            }
            if f.pos >= f.data.len() {
                i = -1;
                break;
            }
            i = i32::from(f.data[f.pos]);
            f.pos += 1;
            if i == i32::from(b'\n') || i == i32::from(b'\r') {
                break;
            }
            self.buffer[self.last as usize] = i as u8;
            self.last += 1;
        }
        if i == -1 && self.last == self.first {
            return Ok(false);
        }
        if i != -1 && i != i32::from(b'\n') && i != i32::from(b'\r') {
            // (the line is longer than the buffer: web2c's uexit(1))
            self.term_out.extend_from_slice(
                format!("! Unable to read an entire line---bufsize={}.\nPlease increase buf_size in texmf.cnf.\n", self.buf_size).as_bytes(),
            );
            self.history = FATAL_ERROR_STOP;
            return Err(Stop);
        }
        self.buffer[self.last as usize] = b' ';
        if self.last >= self.max_buf_stack {
            self.max_buf_stack = self.last;
        }
        if i == i32::from(b'\r') && f.pos < f.data.len() && f.data[f.pos] == b'\n' {
            f.pos += 1;
        }
        while self.last > self.first && self.buffer[(self.last - 1) as usize] == b' ' {
            self.last -= 1;
        }
        for k in self.first..=self.last {
            self.buffer[k as usize] = self.xord[self.buffer[k as usize] as usize];
        }
        Ok(true)
    }

    /// `input_ln` on input file number `n` (`input_file[n]`).
    pub(crate) fn input_ln_file(&mut self, n: usize) -> R<bool> {
        let mut f = std::mem::take(&mut self.input_file[n]);
        let r = self.input_ln_from(&mut f);
        self.input_file[n] = f;
        r
    }

    /// `input_ln(term_in, true)`.
    pub(crate) fn input_ln_term(&mut self) -> R<bool> {
        let mut f = std::mem::take(&mut self.term_in);
        let r = self.input_ln_from(&mut f);
        self.term_in = f;
        r
    }

    // §33: `update_terminal` is a flush; the terminal is kept in memory.
    #[inline]
    pub(crate) fn update_terminal(&mut self) {}

    pub(crate) fn wterm(&mut self, s: &[u8]) {
        self.term_out.extend_from_slice(s);
    }
    pub(crate) fn wterm_ln(&mut self, s: &[u8]) {
        self.term_out.extend_from_slice(s);
        self.term_out.push(b'\n');
    }
    pub(crate) fn wterm_cr(&mut self) {
        self.term_out.push(b'\n');
    }
    pub(crate) fn wlog(&mut self, s: &[u8]) {
        self.log_file.extend_from_slice(s);
    }
    pub(crate) fn wlog_cr(&mut self) {
        self.log_file.push(b'\n');
    }

    // §36 (mf.ch, and web2c's `topenin`)
    pub(crate) fn t_open_in(&mut self) {
        let first = self.first as usize;
        self.buffer[first] = 0;
        let mut k = first;
        let args = std::mem::take(&mut self.cfg.command_line);
        for a in &args {
            for &c in a {
                self.buffer[k] = c;
                k += 1;
            }
            self.buffer[k] = b' ';
            k += 1;
        }
        self.buffer[k] = 0;
        let mut last = first;
        while self.buffer[last] != 0 {
            last += 1;
        }
        let mut l = last as i64 - 1;
        while l >= first as i64 && matches!(self.buffer[l as usize], b' ' | b'\r' | b'\n') {
            l -= 1;
        }
        self.last = (l + 1) as i32;
        for i in self.first..self.last {
            self.buffer[i as usize] = self.xord[self.buffer[i as usize] as usize];
        }
    }

    pub(crate) fn init_terminal(&mut self) -> R<bool> {
        self.t_open_in();
        if self.last > self.first {
            self.set_loc(self.first);
            while self.loc() < self.last && self.buffer[self.loc() as usize] == b' ' {
                self.set_loc(self.loc() + 1);
            }
            if self.loc() < self.last {
                return Ok(true);
            }
        }
        loop {
            self.wterm(b"**");
            self.update_terminal();
            if !self.input_ln_term()? {
                self.wterm_cr();
                self.wterm_ln(b"! End of file on the terminal... why?");
                return Ok(false);
            }
            self.set_loc(self.first);
            while self.loc() < self.last && self.buffer[self.loc() as usize] == b' ' {
                self.set_loc(self.loc() + 1);
            }
            if self.loc() < self.last {
                return Ok(true);
            }
            self.wterm_ln(b"Please type the name of your input file.");
        }
    }

    // §39-§44
    #[inline]
    pub(crate) fn length(&self, s: StrNumber) -> i32 {
        self.str_start[(s + 1) as usize] - self.str_start[s as usize]
    }
    #[inline]
    pub(crate) fn cur_length(&self) -> i32 {
        self.pool_ptr - self.str_start[self.str_ptr as usize]
    }
    #[inline]
    pub(crate) fn append_char(&mut self, c: u8) {
        self.str_pool[self.pool_ptr as usize] = c;
        self.pool_ptr += 1;
    }
    pub(crate) fn str_room(&mut self, n: i32) -> R {
        if self.pool_ptr + n > self.max_pool_ptr {
            if self.pool_ptr + n > POOL_SIZE {
                return self.overflow("pool size", POOL_SIZE - self.init_pool_ptr);
            }
            self.max_pool_ptr = self.pool_ptr + n;
        }
        Ok(())
    }
    pub(crate) fn add_str_ref(&mut self, s: StrNumber) {
        if self.str_ref[s as usize] < 127 {
            self.str_ref[s as usize] += 1;
        }
    }
    pub(crate) fn delete_str_ref(&mut self, s: StrNumber) {
        if self.str_ref[s as usize] < 127 {
            if self.str_ref[s as usize] > 1 {
                self.str_ref[s as usize] -= 1;
            } else {
                self.flush_string(s);
            }
        }
    }
    pub(crate) fn flush_string(&mut self, s: StrNumber) {
        if s < self.str_ptr - 1 {
            self.str_ref[s as usize] = 0;
        } else {
            loop {
                self.str_ptr -= 1;
                if self.str_ref[(self.str_ptr - 1) as usize] != 0 {
                    break;
                }
            }
        }
        self.pool_ptr = self.str_start[self.str_ptr as usize];
    }
    pub(crate) fn make_string(&mut self) -> R<StrNumber> {
        if self.str_ptr == self.max_str_ptr {
            if self.str_ptr == MAX_STRINGS {
                return self.overflow("number of strings", MAX_STRINGS - self.init_str_ptr);
            }
            self.max_str_ptr += 1;
        }
        self.str_ref[self.str_ptr as usize] = 1;
        self.str_ptr += 1;
        self.str_start[self.str_ptr as usize] = self.pool_ptr;
        Ok(self.str_ptr - 1)
    }
    // §45
    pub(crate) fn str_eq_buf(&self, s: StrNumber, mut k: i32) -> bool {
        let mut j = self.str_start[s as usize];
        while j < self.str_start[(s + 1) as usize] {
            if self.str_pool[j as usize] != self.buffer[k as usize] {
                return false;
            }
            j += 1;
            k += 1;
        }
        true
    }
    // §46
    pub(crate) fn str_vs_str(&self, s: StrNumber, t: StrNumber) -> i32 {
        let ls = self.length(s);
        let lt = self.length(t);
        let mut l = if ls <= lt { ls } else { lt };
        let mut j = self.str_start[s as usize];
        let mut k = self.str_start[t as usize];
        while l > 0 {
            if self.str_pool[j as usize] != self.str_pool[k as usize] {
                return i32::from(self.str_pool[j as usize]) - i32::from(self.str_pool[k as usize]);
            }
            j += 1;
            k += 1;
            l -= 1;
        }
        ls - lt
    }

    // §47 (with mf-binpool.ch: the pool compiled in)
    pub(crate) fn get_strings_started(&mut self) -> R<bool> {
        self.pool_ptr = 0;
        self.str_ptr = 0;
        self.max_pool_ptr = 0;
        self.max_str_ptr = 0;
        self.str_start[0] = 0;
        // §48
        for k in 0..256i32 {
            if k < i32::from(b' ') || k > i32::from(b'~') {
                self.append_char(b'^');
                self.append_char(b'^');
                if k < 0o100 {
                    self.append_char((k + 0o100) as u8);
                } else if k < 0o200 {
                    self.append_char((k - 0o100) as u8);
                } else {
                    self.app_lc_hex(k / 16);
                    self.app_lc_hex(k % 16);
                }
            } else {
                self.append_char(k as u8);
            }
            let g = self.make_string()?;
            self.str_ref[g as usize] = 127;
        }
        // (loadpoolstrings)
        let spare = POOL_SIZE - STRING_VACANCIES;
        let mut i = 0;
        for s in crate::hash::pool_strings() {
            let l = s.len() as i32;
            i += l;
            if i >= spare {
                self.wterm_ln(b"! You have to increase POOLSIZE.");
                return Ok(false);
            }
            for &c in s.as_bytes() {
                self.append_char(c);
            }
            let g = self.make_string()?;
            self.str_ref[g as usize] = 127;
        }
        Ok(true)
    }
    pub(crate) fn app_lc_hex(&mut self, l: i32) {
        if l < 10 {
            self.append_char((l + i32::from(b'0')) as u8);
        } else {
            self.append_char((l - 10 + i32::from(b'a')) as u8);
        }
    }

    // §57
    pub(crate) fn print_ln(&mut self) {
        match self.selector {
            TERM_AND_LOG => {
                self.wterm_cr();
                self.wlog_cr();
                self.term_offset = 0;
                self.file_offset = 0;
            }
            LOG_ONLY => {
                self.wlog_cr();
                self.file_offset = 0;
            }
            TERM_ONLY => {
                self.wterm_cr();
                self.term_offset = 0;
            }
            _ => {}
        }
    }
    // §58
    pub(crate) fn print_char(&mut self, s: u8) {
        let c = self.xchr[s as usize];
        match self.selector {
            TERM_AND_LOG => {
                self.term_out.push(c);
                self.log_file.push(c);
                self.term_offset += 1;
                self.file_offset += 1;
                if self.term_offset == self.max_print_line {
                    self.wterm_cr();
                    self.term_offset = 0;
                }
                if self.file_offset == self.max_print_line {
                    self.wlog_cr();
                    self.file_offset = 0;
                }
            }
            LOG_ONLY => {
                self.log_file.push(c);
                self.file_offset += 1;
                if self.file_offset == self.max_print_line {
                    self.print_ln();
                }
            }
            TERM_ONLY => {
                self.term_out.push(c);
                self.term_offset += 1;
                if self.term_offset == self.max_print_line {
                    self.print_ln();
                }
            }
            NO_PRINT => {}
            PSEUDO => {
                if self.tally < self.trick_count {
                    self.trick_buf[(self.tally % self.error_line) as usize] = s;
                }
            }
            NEW_STRING => {
                if self.pool_ptr < POOL_SIZE {
                    self.append_char(s);
                }
            }
            _ => {}
        }
        self.tally += 1;
    }
    // §59 (mf.ch: printable characters as themselves)
    pub(crate) fn print(&mut self, s: i32) {
        let s = if s < 0 || s >= self.str_ptr { 259 } else { s };
        if s < 256 && (self.selector > PSEUDO || self.xprn[s as usize] != 0) {
            self.print_char(s as u8);
        } else {
            let mut j = self.str_start[s as usize];
            while j < self.str_start[(s + 1) as usize] {
                self.print_char(self.str_pool[j as usize]);
                j += 1;
            }
        }
    }
    /// `print` of a string constant.
    pub(crate) fn prints(&mut self, s: &str) {
        let b = s.as_bytes();
        if b.len() == 1 {
            self.print(i32::from(b[0]));
        } else {
            for &c in b {
                self.print_char(c);
            }
        }
    }
    // §60
    pub(crate) fn slow_print(&mut self, s: i32) {
        let s = if s < 0 || s >= self.str_ptr { 259 } else { s };
        if s < 256 && (self.selector > PSEUDO || self.xprn[s as usize] != 0) {
            self.print_char(s as u8);
        } else {
            let mut j = self.str_start[s as usize];
            while j < self.str_start[(s + 1) as usize] {
                self.print(i32::from(self.str_pool[j as usize]));
                j += 1;
            }
        }
    }
    // §62
    pub(crate) fn print_nl(&mut self, s: &str) {
        self.print_nl_start();
        self.prints(s);
    }
    pub(crate) fn print_nl_start(&mut self) {
        if (self.term_offset > 0 && self.selector & 1 == 1)
            || (self.file_offset > 0 && self.selector >= LOG_ONLY)
        {
            self.print_ln();
        }
    }
    // §63
    pub(crate) fn print_the_digs(&mut self, mut k: usize) {
        while k > 0 {
            k -= 1;
            self.print_char(b'0' + self.dig[k]);
        }
    }
    // §64
    pub(crate) fn print_int(&mut self, mut n: i32) {
        let mut k = 0usize;
        if n < 0 {
            self.print_char(b'-');
            if n > -100_000_000 {
                n = -n;
            } else {
                let mut m = -1 - n;
                n = m / 10;
                m = (m % 10) + 1;
                k = 1;
                if m < 10 {
                    self.dig[0] = m as u8;
                } else {
                    self.dig[0] = 0;
                    n += 1;
                }
            }
        }
        loop {
            self.dig[k] = (n % 10) as u8;
            n /= 10;
            k += 1;
            if n == 0 {
                break;
            }
        }
        self.print_the_digs(k);
    }
    // §65
    pub(crate) fn print_dd(&mut self, n: i32) {
        let n = n.abs() % 100;
        self.print_char(b'0' + (n / 10) as u8);
        self.print_char(b'0' + (n % 10) as u8);
    }
    // §66
    pub(crate) fn prompt_input(&mut self, s: &str) -> R {
        self.prints(s);
        self.term_input()
    }
    pub(crate) fn term_input(&mut self) -> R {
        self.update_terminal();
        if !self.input_ln_term()? {
            return self.fatal_error("End of file on the terminal!");
        }
        self.term_offset = 0;
        self.selector -= 1;
        if self.last != self.first {
            for k in self.first..self.last {
                self.print(i32::from(self.buffer[k as usize]));
            }
        }
        self.print_ln();
        self.buffer[self.last as usize] = b'%';
        self.selector += 1;
        Ok(())
    }

    // §68 (mf.ch: file:line:error style)
    pub(crate) fn print_err(&mut self, s: &str) {
        if self.cfg.file_line_error_style && !self.terminal_input() {
            self.print_nl("");
            let f = self.full_source_filename_stack[self.in_open as usize];
            self.print(f);
            self.prints(":");
            self.print_int(self.line);
            self.prints(": ");
            self.prints(s);
        } else {
            self.print_nl("! ");
            self.prints(s);
        }
    }
    /// The help lines, `help_line[0]` the last.
    pub(crate) fn help(&mut self, lines: &[&'static str]) {
        self.help_ptr = lines.len();
        for (k, l) in lines.iter().rev().enumerate() {
            self.help_line[k] = l;
        }
    }

    // §76 (mf.ch)
    pub(crate) fn jump_out<T>(&mut self) -> R<T> {
        let _ = self.close_files_and_terminate();
        self.do_final_end()
    }
    pub(crate) fn do_final_end<T>(&mut self) -> R<T> {
        self.update_terminal();
        self.ready_already = 0;
        Err(Stop)
    }

    // §77
    pub(crate) fn error(&mut self) -> R {
        if self.history < ERROR_MESSAGE_ISSUED {
            self.history = ERROR_MESSAGE_ISSUED;
        }
        self.print_char(b'.');
        self.show_context()?;
        if self.cfg.halt_on_error {
            if self.halting_on_error_p {
                return self.do_final_end();
            }
            self.halting_on_error_p = true;
            self.history = FATAL_ERROR_STOP;
            return self.jump_out();
        }
        if self.interaction == ERROR_STOP_MODE {
            // §78
            loop {
                if self.interaction != ERROR_STOP_MODE {
                    return Ok(());
                }
                self.clear_for_error_prompt()?;
                self.prompt_input("? ")?;
                if self.last == self.first {
                    return Ok(());
                }
                let mut c = self.buffer[self.first as usize];
                if c >= b'a' {
                    c = c.wrapping_add(b'A').wrapping_sub(b'a');
                }
                // §79
                match c {
                    b'0'..=b'9' if self.deletions_allowed => {
                        // §83
                        let s1 = self.cur_cmd;
                        let s2 = self.cur_mod;
                        let s3 = self.cur_sym;
                        self.ok_to_interrupt = false;
                        let mut c = i32::from(c);
                        if self.last > self.first + 1
                            && self.buffer[(self.first + 1) as usize] >= b'0'
                            && self.buffer[(self.first + 1) as usize] <= b'9'
                        {
                            c = c * 10 + i32::from(self.buffer[(self.first + 1) as usize])
                                - i32::from(b'0') * 11;
                        } else {
                            c -= i32::from(b'0');
                        }
                        while c > 0 {
                            self.get_next()?;
                            // §743
                            if self.cur_cmd == STRING_TOKEN {
                                self.delete_str_ref(self.cur_mod);
                            }
                            c -= 1;
                        }
                        self.cur_cmd = s1;
                        self.cur_mod = s2;
                        self.cur_sym = s3;
                        self.ok_to_interrupt = true;
                        self.help(&[
                            "I have just deleted some text, as you asked.",
                            "You can now delete more, or insert, or whatever.",
                        ]);
                        self.show_context()?;
                        continue;
                    }
                    b'E' => {
                        if self.file_ptr > 0
                            && self.input_stack[self.file_ptr as usize].name_field >= 256
                        {
                            let n = self.input_stack[self.file_ptr as usize].name_field;
                            self.edit_name_start = self.str_start[n as usize];
                            self.edit_name_length =
                                self.str_start[(n + 1) as usize] - self.str_start[n as usize];
                            self.edit_line = self.line;
                            return self.jump_out();
                        }
                    }
                    b'H' => {
                        // §84
                        if self.use_err_help {
                            self.print_err_help();
                            self.use_err_help = false;
                        } else {
                            if self.help_ptr == 0 {
                                self.help(&[
                                    "Sorry, I don't know how to help in this situation.",
                                    "Maybe you should try asking a human?",
                                ]);
                            }
                            loop {
                                self.help_ptr -= 1;
                                let l = self.help_line[self.help_ptr];
                                self.prints(l);
                                self.print_ln();
                                if self.help_ptr == 0 {
                                    break;
                                }
                            }
                        }
                        self.help(&[
                            "Sorry, I already gave what help I could...",
                            "Maybe you should try asking a human?",
                            "An error might have occurred before I noticed any problems.",
                            "``If all else fails, read the instructions.''",
                        ]);
                        continue;
                    }
                    b'I' => {
                        // §82
                        self.begin_file_reading()?;
                        if self.last > self.first + 1 {
                            self.set_loc(self.first + 1);
                            self.buffer[self.first as usize] = b' ';
                        } else {
                            self.prompt_input("insert>")?;
                            self.set_loc(self.first);
                        }
                        self.first = self.last + 1;
                        self.set_limit(self.last);
                        return Ok(());
                    }
                    b'Q' | b'R' | b'S' => {
                        // §81
                        self.error_count = 0;
                        self.interaction = BATCH_MODE + i32::from(c) - i32::from(b'Q');
                        self.prints("OK, entering ");
                        match c {
                            b'Q' => {
                                self.prints("batchmode");
                                self.selector -= 1;
                            }
                            b'R' => self.prints("nonstopmode"),
                            _ => self.prints("scrollmode"),
                        }
                        self.prints("...");
                        self.print_ln();
                        self.update_terminal();
                        return Ok(());
                    }
                    b'X' => {
                        self.interaction = SCROLL_MODE;
                        return self.jump_out();
                    }
                    _ => {}
                }
                // §80
                self.prints("Type <return> to proceed, S to scroll future error messages,");
                self.print_nl("R to run without stopping, Q to run quietly,");
                self.print_nl("I to insert something, ");
                if self.file_ptr > 0 && self.input_stack[self.file_ptr as usize].name_field >= 256 {
                    self.prints("E to edit your file,");
                }
                if self.deletions_allowed {
                    self.print_nl("1 or ... or 9 to ignore the next 1 to 9 tokens of input,");
                }
                self.print_nl("H for help, X to quit.");
            }
        }
        self.error_count += 1;
        if self.error_count == 100 {
            self.print_nl("(That makes 100 errors; please try again.)");
            self.history = FATAL_ERROR_STOP;
            return self.jump_out();
        }
        // §86
        if self.interaction > BATCH_MODE {
            self.selector -= 1;
        }
        if self.use_err_help {
            self.print_nl("");
            self.print_err_help();
        } else {
            while self.help_ptr > 0 {
                self.help_ptr -= 1;
                let l = self.help_line[self.help_ptr];
                self.print_nl(l);
            }
        }
        self.print_ln();
        if self.interaction > BATCH_MODE {
            self.selector += 1;
        }
        self.print_ln();
        Ok(())
    }
    // §85
    pub(crate) fn print_err_help(&mut self) {
        let e = self.err_help as usize;
        let mut j = self.str_start[e];
        while j < self.str_start[e + 1] {
            if self.str_pool[j as usize] != b'%' {
                self.print(i32::from(self.str_pool[j as usize]));
            } else if j + 1 == self.str_start[e + 1] {
                self.print_ln();
            } else if self.str_pool[(j + 1) as usize] != b'%' {
                self.print_ln();
            } else {
                j += 1;
                self.print_char(b'%');
            }
            j += 1;
        }
    }
    // §87
    pub(crate) fn normalize_selector(&mut self) -> R {
        self.selector = if self.log_opened {
            TERM_AND_LOG
        } else {
            TERM_ONLY
        };
        if self.job_name == 0 {
            self.open_log_file()?;
        }
        if self.interaction == BATCH_MODE {
            self.selector -= 1;
        }
        Ok(())
    }
    // §88
    pub(crate) fn succumb<T>(&mut self) -> R<T> {
        if self.interaction == ERROR_STOP_MODE {
            self.interaction = SCROLL_MODE;
        }
        if self.log_opened {
            self.error()?;
        }
        self.history = FATAL_ERROR_STOP;
        self.jump_out()
    }
    pub(crate) fn fatal_error<T>(&mut self, s: &'static str) -> R<T> {
        self.normalize_selector()?;
        self.print_err("Emergency stop");
        self.help(&[s]);
        self.succumb()
    }
    // §89
    pub(crate) fn overflow<T>(&mut self, s: &str, n: i32) -> R<T> {
        self.normalize_selector()?;
        self.print_err("METAFONT capacity exceeded, sorry [");
        self.prints(s);
        self.print_char(b'=');
        self.print_int(n);
        self.print_char(b']');
        self.help(&[
            "If you really absolutely need more capacity,",
            "you can ask a wizard to enlarge me.",
        ]);
        self.succumb()
    }
    // §90
    pub(crate) fn confusion<T>(&mut self, s: &str) -> R<T> {
        self.normalize_selector()?;
        if self.history < ERROR_MESSAGE_ISSUED {
            self.print_err("This can't happen (");
            self.prints(s);
            self.print_char(b')');
            self.help(&["I'm broken. Please show this to someone who can fix can fix"]);
        } else {
            self.print_err("I can't go on meeting you like this");
            self.help(&[
                "One of your faux pas seems to have wounded me deeply...",
                "in fact, I'm barely conscious. Please fix it and try again.",
            ]);
        }
        self.succumb()
    }
    // §91
    #[inline]
    pub(crate) fn check_interrupt(&mut self) -> R {
        if self.interrupt != 0 {
            self.pause_for_instructions()?;
        }
        Ok(())
    }
    // §93
    pub(crate) fn pause_for_instructions(&mut self) -> R {
        if self.ok_to_interrupt {
            self.interaction = ERROR_STOP_MODE;
            if self.selector == LOG_ONLY || self.selector == NO_PRINT {
                self.selector += 1;
            }
            self.print_err("Interruption");
            self.help(&[
                "You rang?",
                "Try to insert an instruction for me (e.g., `I show x;'),",
                "unless you just want to quit by typing `X'.",
            ]);
            self.deletions_allowed = false;
            self.error()?;
            self.deletions_allowed = true;
            self.interrupt = 0;
        }
        Ok(())
    }
    // §94
    pub(crate) fn missing_err(&mut self, s: &str) {
        self.print_err("Missing `");
        self.prints(s);
        self.prints("' has been inserted");
    }

    // §99
    pub(crate) fn check_arith(&mut self) -> R {
        if self.arith_error {
            self.clear_arith()?;
        }
        Ok(())
    }
    pub(crate) fn clear_arith(&mut self) -> R {
        self.print_err("Arithmetic overflow");
        self.help(&[
            "Uh, oh. A little while ago one of the quantities that I was",
            "computing got too large, so I'm afraid your answers will be",
            "somewhat askew. You'll probably have to adopt different",
            "tactics next time. But I shall try to carry on anyway.",
        ]);
        self.error()?;
        self.arith_error = false;
        Ok(())
    }

    // §103
    pub(crate) fn print_scaled(&mut self, mut s: i32) {
        if s < 0 {
            self.print_char(b'-');
            s = -s;
        }
        self.print_int(s / UNITY);
        s = 10 * (s % UNITY) + 5;
        if s != 5 {
            let mut delta = 10;
            self.print_char(b'.');
            loop {
                if delta > UNITY {
                    s = s + 0o100000 - (delta / 2);
                }
                self.print_char(b'0' + (s / UNITY) as u8);
                s = 10 * (s % UNITY);
                delta *= 10;
                if s <= delta {
                    break;
                }
            }
        }
    }
    // §104
    pub(crate) fn print_two(&mut self, x: i32, y: i32) {
        self.print_char(b'(');
        self.print_scaled(x);
        self.print_char(b',');
        self.print_scaled(y);
        self.print_char(b')');
    }

    // §195
    pub(crate) fn begin_diagnostic(&mut self) {
        self.old_setting = self.selector;
        if self.int(TRACING_ONLINE) <= 0 && self.selector == TERM_AND_LOG {
            self.selector -= 1;
            if self.history == SPOTLESS {
                self.history = WARNING_ISSUED;
            }
        }
    }
    pub(crate) fn end_diagnostic(&mut self, blank_line: bool) {
        self.print_nl("");
        if blank_line {
            self.print_ln();
        }
        self.selector = self.old_setting;
    }
    // §197
    pub(crate) fn print_diagnostic(&mut self, s: &str, t: &str, nuline: bool) {
        self.begin_diagnostic();
        if nuline {
            self.print_nl(s);
        } else {
            self.prints(s);
        }
        self.prints(" at line ");
        self.print_int(self.line);
        self.prints(t);
        self.print_char(b':');
    }
}
