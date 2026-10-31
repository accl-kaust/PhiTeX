//! Part 38 of `mf.web` with `mf.ch`: file names, the log file, input.
//! Files are found by [`crate::Io`]; output files are collected in
//! [`Mf::outputs`] when closed.

use crate::print::BANNER;
use crate::{FileKind, Mf, R, StrNumber, consts::*};

impl Mf {
    // §769
    pub(crate) fn begin_name(&mut self) {
        self.area_delimiter = 0;
        self.ext_delimiter = 0;
        self.quoted_filename = false;
    }
    // §770 (mf.ch)
    pub(crate) fn more_name(&mut self, c: u8) -> R<bool> {
        if c == b'"' {
            self.quoted_filename = !self.quoted_filename;
            Ok(true)
        } else if (c == b' ' || c == TAB) && self.stop_at_space && !self.quoted_filename {
            Ok(false)
        } else {
            if c == b'/' {
                self.area_delimiter = self.pool_ptr;
                self.ext_delimiter = 0;
            } else if c == b'.' {
                self.ext_delimiter = self.pool_ptr;
            }
            self.str_room(1)?;
            self.append_char(c);
            Ok(true)
        }
    }
    // (mf.ch: pool_seq_check, pool_seq_quote_move, pool_seq_quote)
    fn pool_seq_check(&self, t: i32) -> (bool, i32) {
        let s = self.str_start[self.str_ptr as usize];
        let mut j = s;
        let mut must_quote = false;
        while !must_quote && j < t {
            must_quote = self.str_pool[j as usize] == b' ';
            j += 1;
        }
        (must_quote, s)
    }
    fn pool_seq_quote(&mut self, s: i32, t: i32) {
        self.str_pool[(t + 1) as usize] = b'"';
        let mut j = t - 1;
        while j >= s {
            self.str_pool[(j + 1) as usize] = self.str_pool[j as usize];
            j -= 1;
        }
        self.str_pool[s as usize] = b'"';
        self.pool_ptr += 2;
    }
    fn pool_seq_quote_move(&mut self, s: i32, t: i32) {
        let mut j = self.pool_ptr - 1;
        while j >= t {
            self.str_pool[(j + 2) as usize] = self.str_pool[j as usize];
            j -= 1;
        }
        self.pool_seq_quote(s, t);
    }
    // §771 (mf.ch)
    pub(crate) fn end_name(&mut self) -> R {
        if self.str_ptr + 3 > self.max_str_ptr {
            if self.str_ptr + 3 > MAX_STRINGS {
                return self.overflow("number of strings", MAX_STRINGS - self.init_str_ptr);
            }
            self.max_str_ptr = self.str_ptr + 3;
        }
        self.str_room(6)?;
        if self.area_delimiter == 0 {
            self.cur_area = self.pstr("");
        } else {
            let (mq, s) = self.pool_seq_check(self.area_delimiter + 1);
            if mq {
                self.pool_seq_quote_move(s, self.area_delimiter + 1);
                self.area_delimiter += 2;
                if self.ext_delimiter != 0 {
                    self.ext_delimiter += 2;
                }
            }
            self.cur_area = self.str_ptr;
            self.str_ptr += 1;
            self.str_start[self.str_ptr as usize] = self.area_delimiter + 1;
        }
        if self.ext_delimiter == 0 {
            self.cur_ext = self.pstr("");
        } else {
            let (mq, s) = self.pool_seq_check(self.ext_delimiter);
            if mq {
                self.pool_seq_quote_move(s, self.ext_delimiter);
                self.ext_delimiter += 2;
            }
            self.cur_name = self.str_ptr;
            self.str_ptr += 1;
            self.str_start[self.str_ptr as usize] = self.ext_delimiter;
        }
        let (mq, s) = self.pool_seq_check(self.pool_ptr);
        if mq {
            self.pool_seq_quote(s, self.pool_ptr);
        }
        if self.ext_delimiter == 0 {
            self.cur_name = self.make_string()?;
        } else {
            self.cur_ext = self.make_string()?;
        }
        Ok(())
    }

    // §773 (mf.ch)
    pub(crate) fn print_file_name(&mut self, n: StrNumber, a: StrNumber, e: StrNumber) {
        let mut must_quote = false;
        for s in [a, n, e] {
            if s != 0 {
                let mut j = self.str_start[s as usize];
                while !must_quote && j < self.str_start[(s + 1) as usize] {
                    must_quote = self.str_pool[j as usize] == b' ';
                    j += 1;
                }
            }
        }
        if must_quote {
            self.slow_print(i32::from(b'"'));
        }
        for s in [a, n, e] {
            if s != 0 {
                for j in self.str_start[s as usize]..self.str_start[(s + 1) as usize] {
                    let c = self.str_pool[j as usize];
                    if c != b'"' {
                        self.print(i32::from(c));
                    }
                }
            }
        }
        if must_quote {
            self.slow_print(i32::from(b'"'));
        }
    }

    // §774 (mf.ch)
    pub(crate) fn pack_file_name(&mut self, n: StrNumber, a: StrNumber, e: StrNumber) {
        let mut name = Vec::new();
        for s in [a, n, e] {
            for j in self.str_start[s as usize]..self.str_start[(s + 1) as usize] {
                let c = self.str_pool[j as usize];
                if c != b'"' {
                    name.push(self.xchr[c as usize]);
                }
            }
        }
        self.name_length = name.len() as i32;
        self.name_of_file = name;
    }

    // §778 (mf.ch): `MF_base_default` is " NAME.base", 1-based.
    pub(crate) fn pack_buffered_name(&mut self, n: i32, a: i32, b: i32) {
        let d = self.mf_base_default.clone();
        let bdl = d.len() as i32 - 1;
        let mut name = Vec::new();
        for j in 1..=n {
            let c = self.xord[d[j as usize] as usize];
            if c != b'"' {
                name.push(self.xchr[c as usize]);
            }
        }
        for j in a..=b {
            let c = self.buffer[j as usize];
            if c != b'"' {
                name.push(self.xchr[c as usize]);
            }
        }
        for j in bdl - BASE_EXT_LENGTH + 1..=bdl {
            let c = self.xord[d[j as usize] as usize];
            if c != b'"' {
                name.push(self.xchr[c as usize]);
            }
        }
        self.name_length = name.len() as i32;
        self.name_of_file = name;
    }

    /// `w_open_in(base_file)`.
    fn w_open_in(&mut self) -> bool {
        let name = self.name_of_file.clone();
        match self.io.open_in(&name, FileKind::Base) {
            Some((data, found)) => {
                self.base_file = data;
                self.base_pos = 0;
                self.name_length = found.len() as i32;
                self.name_of_file = found;
                true
            }
            None => false,
        }
    }

    // §779 (mf.ch)
    pub(crate) fn open_base_file(&mut self) -> R<bool> {
        let mut j = self.loc();
        'found: {
            if self.buffer[self.loc() as usize] == b'&' {
                self.set_loc(self.loc() + 1);
                j = self.loc();
                self.buffer[self.last as usize] = b' ';
                while self.buffer[j as usize] != b' ' {
                    j += 1;
                }
                self.pack_buffered_name(0, self.loc(), j - 1);
                if self.w_open_in() {
                    break 'found;
                }
                let mut s = b"Sorry, I can't find the base `".to_vec();
                s.extend_from_slice(&self.name_of_file);
                s.extend_from_slice(b"'; will try `");
                s.extend_from_slice(&self.mf_base_default[1..]);
                s.extend_from_slice(b"'.");
                self.wterm_ln(&s);
            }
            let bdl = self.mf_base_default.len() as i32 - 1;
            self.pack_buffered_name(bdl - BASE_EXT_LENGTH, 1, 0);
            if !self.w_open_in() {
                let mut s = b"I can't find the base file `".to_vec();
                s.extend_from_slice(&self.mf_base_default[1..]);
                s.extend_from_slice(b"'!");
                self.wterm_ln(&s);
                return Ok(false);
            }
        }
        self.set_loc(j);
        Ok(true)
    }

    // §780 (mf.ch)
    pub(crate) fn make_name_string(&mut self) -> R<StrNumber> {
        let r;
        if self.pool_ptr + self.name_length > POOL_SIZE || self.str_ptr == MAX_STRINGS {
            r = i32::from(b'?');
        } else {
            for k in 0..self.name_length as usize {
                let c = self.xord[self.name_of_file[k] as usize];
                self.append_char(c);
            }
            r = self.make_string()?;
        }
        let mut k = 0usize;
        self.begin_name();
        self.stop_at_space = false;
        while k < self.name_length as usize {
            let c = self.name_of_file[k];
            if !self.more_name(c)? {
                break;
            }
            k += 1;
        }
        self.stop_at_space = true;
        self.end_name()?;
        Ok(r)
    }

    // §787
    pub(crate) fn scan_file_name(&mut self) -> R {
        self.begin_name();
        while self.buffer[self.loc() as usize] == b' ' || self.buffer[self.loc() as usize] == TAB {
            self.set_loc(self.loc() + 1);
        }
        loop {
            let c = self.buffer[self.loc() as usize];
            if c == b';' || c == b'%' {
                break;
            }
            if !self.more_name(c)? {
                break;
            }
            self.set_loc(self.loc() + 1);
        }
        self.end_name()
    }

    // §784
    pub(crate) fn pack_job_name(&mut self, s: StrNumber) {
        self.cur_area = self.pstr("");
        self.cur_ext = s;
        self.cur_name = self.job_name;
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
    }

    // §786
    pub(crate) fn prompt_file_name(&mut self, s: &str, e: &str) -> R {
        if s == "input file name" {
            self.print_err("I can't find file `");
        } else {
            self.print_err("I can't write on file `");
        }
        self.print_file_name(self.cur_name, self.cur_area, self.cur_ext);
        self.prints("'.");
        if e == ".mf" {
            self.show_context()?;
        }
        self.print_nl("Please type another ");
        self.prints(s);
        if self.interaction < SCROLL_MODE {
            return self.fatal_error("*** (job aborted, file error in nonstop mode)");
        }
        let saved_cur_name = self.cur_name;
        self.prompt_input(": ")?;
        // §787
        self.begin_name();
        let mut k = self.first;
        while (self.buffer[k as usize] == b' ' || self.buffer[k as usize] == TAB) && k < self.last {
            k += 1;
        }
        loop {
            if k == self.last {
                break;
            }
            if !self.more_name(self.buffer[k as usize])? {
                break;
            }
            k += 1;
        }
        self.end_name()?;
        if self.cur_ext == self.pstr("") {
            self.cur_ext = self.pstr(e);
        }
        if self.length(self.cur_name) == 0 {
            self.cur_name = saved_cur_name;
        }
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
        Ok(())
    }

    /// `get_job_name` (lib/texmfmp.c): the `-jobname` given, else `name`.
    fn get_job_name(&mut self, name: StrNumber) -> R<StrNumber> {
        match self.cfg.job_name.clone() {
            Some(j) => {
                self.str_room(j.len() as i32)?;
                for c in j.bytes() {
                    self.append_char(c);
                }
                self.make_string()
            }
            None => Ok(name),
        }
    }

    // §788 (mf.ch)
    pub(crate) fn open_log_file(&mut self) -> R {
        let old_setting = self.selector;
        if self.job_name == 0 {
            self.job_name = self.get_job_name(self.pstr("mfput"))?;
        }
        self.pack_job_name(self.pstr(".log"));
        self.log_file = Vec::new();
        self.log_file_name = self.name_of_file.clone();
        self.log_name = self.make_name_string()?;
        self.selector = LOG_ONLY;
        self.log_opened = true;
        // §790
        self.wlog(BANNER.as_bytes());
        self.slow_print(self.base_ident);
        self.prints("  ");
        self.print_int(self.sys_day);
        self.print_char(b' ');
        let months = b" JANFEBMARAPRMAYJUNJULAUGSEPOCTNOVDEC";
        for k in 3 * self.sys_month - 2..=3 * self.sys_month {
            self.wlog(&months[k as usize..=k as usize]);
        }
        self.print_char(b' ');
        self.print_int(self.sys_year);
        self.print_char(b' ');
        self.print_dd(self.sys_time / 60);
        self.print_char(b':');
        self.print_dd(self.sys_time % 60);
        if let Some(t) = self.cfg.translate.as_ref().map(|t| t.0.clone()) {
            self.wlog_cr();
            self.wlog(b"(");
            self.wlog(t.as_bytes());
            self.wlog(b")");
        }
        self.input_stack[self.input_ptr as usize] = self.cur_input;
        self.print_nl("**");
        let l = self.input_stack[0].limit_field - 1;
        for k in 1..=l {
            self.print(i32::from(self.buffer[k as usize]));
        }
        self.print_ln();
        self.selector = old_setting + 2;
        Ok(())
    }

    // §791
    pub(crate) fn set_output_file_name(&mut self) -> R {
        if self.job_name == 0 {
            self.open_log_file()?;
        }
        self.pack_job_name(self.gf_ext);
        self.gf_file = Vec::new();
        self.gf_file_name = self.name_of_file.clone();
        self.output_file_name = self.make_name_string()?;
        Ok(())
    }

    // §793 (mf.ch)
    pub(crate) fn start_input(&mut self) -> R {
        // §795
        while self.token_state() && self.loc() == NULL {
            self.end_token_list()?;
        }
        if self.token_state() {
            self.print_err("File names can't appear within macros");
            self.help(&[
                "Sorry...I've converted what follows to tokens,",
                "possibly garbaging the name you gave.",
                "Please delete the tokens and insert the name again.",
            ]);
            self.error()?;
        }
        if self.file_state() {
            self.scan_file_name()?;
        } else {
            self.cur_name = self.pstr("");
            self.cur_ext = self.pstr("");
            self.cur_area = self.pstr("");
        }
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
        loop {
            self.begin_file_reading()?;
            if self.cur_ext == self.pstr(".mf") {
                self.cur_ext = self.pstr("");
                self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
            }
            let name = self.name_of_file.clone();
            if let Some((data, found)) = self.io.open_in(&name, FileKind::Mf) {
                let i = self.index() as usize;
                self.input_file[i] = crate::AlphaFile { data, pos: 0 };
                self.name_length = found.len() as i32;
                self.name_of_file = found;
                break;
            }
            self.end_file_reading()?;
            self.prompt_file_name("input file name", ".mf")?;
        }
        let n = self.make_name_string()?;
        self.set_name(n);
        self.str_ref[self.cur_name as usize] = MAX_STR_REF as u8;
        if self.job_name == 0 {
            self.job_name = self.get_job_name(self.cur_name)?;
            self.open_log_file()?;
        }
        if self.term_offset + self.length(self.name()) > self.max_print_line - 2 {
            self.print_ln();
        } else if self.term_offset > 0 || self.file_offset > 0 {
            self.print_char(b' ');
        }
        self.print_char(b'(');
        self.open_parens += 1;
        self.slow_print(self.name());
        self.update_terminal();
        // §794
        self.line = 1;
        self.input_ln_file(self.index() as usize)?;
        self.firm_up_the_line()?;
        let lim = self.limit();
        self.buffer[lim as usize] = b'%';
        self.first = lim + 1;
        self.set_loc(self.start());
        Ok(())
    }
}
