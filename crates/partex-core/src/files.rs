//! Part 29: File names (§511–§538), as changed by web2c (quoted names,
//! `\input{...}`, kpathsea lookups through the host).

use alloc::vec::Vec;

use crate::error::SCROLL_MODE;
use crate::host::{FileKind, Host};
use crate::input::{AlphaFile, ux};
use crate::print::{LOG_ONLY, NEW_STRING, TERM_ONLY};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// tex.ch: `TeX_banner`.
pub const BANNER: &[u8] = b"This is TeX, Version 3.141592653";
/// tex.ch: `TeX_banner_k`, printed with web2c's `-file-line-error`.
pub const BANNER_K: &[u8] = b"This is TeXk, Version 3.141592653";
/// pdfTeX §2: `pdfTeX_banner` (also its `banner_k`).
pub const PDFTEX_BANNER: &[u8] = b"This is pdfTeX, Version 3.141592653-2.6-1.40.29";
/// `XeTeX` §2: the banner of `XeTeX` (TeX Live 2026).
pub const XETEX_BANNER: &[u8] = b"This is XeTeX, Version 3.141592653-2.6-0.999998";
/// cpascal.h: `promptfilenamehelpmsg` (Unix).
const PROMPT_FILE_NAME_HELP_MSG: &[u8] = b"(Press Enter to retry, or Control-D to exit";

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §2: the banner of the flavor (web2c's `banner_k` with
    /// `-file-line-error`).
    pub(crate) fn banner(&self) -> &'static [u8] {
        match self.params.flavor {
            crate::params::Flavor::PdfTex => PDFTEX_BANNER,
            crate::params::Flavor::XeTeX => XETEX_BANNER,
            crate::params::Flavor::Tex if self.params.file_line_error_style => BANNER_K,
            crate::params::Flavor::Tex => BANNER,
        }
    }

    /// web2c: `search_string`: an older string equal to `search`, or 0
    /// (through an index of the pool, `strings.rs`).
    pub(crate) fn search_string(&mut self, search: i32) -> i32 {
        let len = self.length(ux(search));
        if len == 0 {
            return self.pool_str(b""); // trivial case
        }
        self.search_string_indexed(ux(search))
    }

    /// web2c: `slow_make_string`: `make_string`, reusing an equal string.
    pub(crate) fn slow_make_string(&mut self) -> Result<i32, Jump> {
        let t = i32::try_from(self.make_string()?).unwrap_or(0);
        let s = self.search_string(t);
        if s > 0 {
            self.flush_string();
            return Ok(s);
        }
        Ok(t)
    }

    /// §515
    pub(crate) fn begin_name(&mut self) {
        self.area_delimiter = 0;
        self.ext_delimiter = 0;
        self.quoted_filename = false;
        self.file_name_quote_char = 0;
    }

    /// §516: add `c` to the name being scanned; false at its end. The
    /// delimiters are where they are in the pool's bytes.
    pub(crate) fn more_name(&mut self, c: u32) -> Result<bool, Jump> {
        if self.unicode {
            return self.more_name_xetex(c);
        }
        if c == u32::from(b' ') && self.stop_at_space && !self.quoted_filename {
            Ok(false)
        } else if c == u32::from(b'"') {
            self.quoted_filename = !self.quoted_filename;
            Ok(true)
        } else {
            self.str_room(1)?;
            self.append_char(c); // contribute `c` to the current string
            self.name_delimiters(c);
            Ok(true)
        }
    }

    /// `XeTeX` §551: a name may be quoted with `"` or `'` (only while
    /// spaces end it); the quote characters are not part of it.
    fn more_name_xetex(&mut self, c: u32) -> Result<bool, Jump> {
        let quote = u32::from(b'"');
        let apostrophe = u32::from(b'\'');
        if self.stop_at_space && c == u32::from(b' ') && self.file_name_quote_char == 0 {
            Ok(false)
        } else if self.stop_at_space
            && self.file_name_quote_char != 0
            && c == self.file_name_quote_char
        {
            self.file_name_quote_char = 0;
            Ok(true)
        } else if self.stop_at_space
            && self.file_name_quote_char == 0
            && (c == quote || c == apostrophe)
        {
            self.file_name_quote_char = c;
            self.quoted_filename = true;
            Ok(true)
        } else {
            self.str_room(if c > 0xFFFF { 2 } else { 1 })?;
            self.append_char(c);
            self.name_delimiters(c);
            Ok(true)
        }
    }

    /// §516's `IS_DIR_SEP` and `.`: the delimiters, as pool offsets.
    fn name_delimiters(&mut self, c: u32) {
        let len = self.pool_ptr - self.str_start[self.str_ptr];
        if c == u32::from(b'/') {
            self.area_delimiter = len;
            self.ext_delimiter = 0;
        } else if c == u32::from(b'.') {
            self.ext_delimiter = len;
        }
    }

    /// §517 (web2c): finish a file name: set `cur_area`, `cur_name`,
    /// `cur_ext`, quoting parts that contain spaces.
    pub(crate) fn end_name(&mut self) -> Result<(), Jump> {
        if self.str_ptr + 3 > self.max_strings() {
            let n = self.max_strings() - self.init_str_ptr;
            return self.overflow(b"number of strings", i32::try_from(n).unwrap_or(0));
        }
        self.str_room(6)?; // room for quotes, if needed
        let base = self.str_start[self.str_ptr];
        // add quotes if needed (not `XeTeX`, §552)
        if self.unicode {
        } else if self.area_delimiter != 0 {
            // maybe quote `cur_area`
            let (s, t) = (base, base + self.area_delimiter);
            if self.str_pool[s..t].contains(&b' ') {
                self.quote_range(s, t, true);
                if self.ext_delimiter != 0 {
                    self.ext_delimiter += 2;
                }
                self.area_delimiter += 2;
            }
        }
        // maybe quote `cur_name`
        let s = base + self.area_delimiter;
        let t = if self.ext_delimiter == 0 {
            self.pool_ptr
        } else {
            base + self.ext_delimiter - 1
        };
        if !self.unicode && self.str_pool[s..t].contains(&b' ') {
            self.quote_range(s, t, true);
            if self.ext_delimiter != 0 {
                self.ext_delimiter += 2;
            }
        }
        if !self.unicode && self.ext_delimiter != 0 {
            // maybe quote `cur_ext`
            let (s, t) = (base + self.ext_delimiter - 1, self.pool_ptr);
            if self.str_pool[s..t].contains(&b' ') {
                self.quote_range(s, t, false);
            }
        }
        if self.area_delimiter == 0 {
            self.cur_area = self.pool_str(b"");
        } else {
            self.cur_area = i32::try_from(self.str_ptr).unwrap_or(0);
            self.grow_starts(self.str_ptr + 1);
            self.str_start[self.str_ptr + 1] = self.str_start[self.str_ptr] + self.area_delimiter;
            self.pool_top_read();
            self.str_ptr += 1;
            self.pool_wrote(self.str_ptr - 1);
            let temp_str = self.search_string(self.cur_area);
            if temp_str > 0 {
                self.cur_area = temp_str;
                self.str_ptr -= 1; // no `flush_string`, `pool_ptr` will be wrong!
                self.pool_wrote(self.str_ptr);
                self.str_index.truncate(self.str_ptr);
                self.strings_reopened();
                for j in self.str_start[self.str_ptr + 1]..self.pool_ptr {
                    self.str_pool[j - self.area_delimiter] = self.str_pool[j];
                }
                self.pool_ptr -= self.area_delimiter; // update `pool_ptr`
            }
        }
        if self.ext_delimiter == 0 {
            self.cur_ext = self.pool_str(b"");
            self.cur_name = self.slow_make_string()?;
        } else {
            self.cur_name = i32::try_from(self.str_ptr).unwrap_or(0);
            self.grow_starts(self.str_ptr + 1);
            self.str_start[self.str_ptr + 1] =
                self.str_start[self.str_ptr] + self.ext_delimiter - self.area_delimiter - 1;
            self.pool_top_read();
            self.str_ptr += 1;
            self.pool_wrote(self.str_ptr - 1);
            self.cur_ext = i32::try_from(self.make_string()?).unwrap_or(0);
            self.str_ptr -= 1; // undo extension string to look at name part
            self.pool_wrote(self.str_ptr);
            self.str_index.truncate(self.str_ptr);
            self.strings_reopened();
            let temp_str = self.search_string(self.cur_name);
            if temp_str > 0 {
                self.cur_name = temp_str;
                self.str_ptr -= 1; // no `flush_string`, `pool_ptr` will be wrong!
                self.pool_wrote(self.str_ptr);
                self.str_index.truncate(self.str_ptr);
                self.strings_reopened();
                let shift = self.ext_delimiter - self.area_delimiter - 1;
                for j in self.str_start[self.str_ptr + 1]..self.pool_ptr {
                    self.str_pool[j - shift] = self.str_pool[j];
                }
                self.pool_ptr -= shift; // update `pool_ptr`
            }
            self.cur_ext = self.slow_make_string()?; // remake extension string
        }
        Ok(())
    }

    /// §517: put quotes around `str_pool[s..t]`, shifting the rest of the
    /// current string (if `shift_rest`) by two.
    fn quote_range(&mut self, s: usize, t: usize, shift_rest: bool) {
        self.grow_pool(2);
        if shift_rest {
            for j in (t..self.pool_ptr).rev() {
                self.str_pool[j + 2] = self.str_pool[j];
            }
        }
        self.str_pool[t + 1] = b'"';
        for j in (s..t).rev() {
            self.str_pool[j + 1] = self.str_pool[j];
        }
        self.str_pool[s] = b'"';
        self.pool_ptr += 2;
    }

    /// The mode and converter a file `XeTeX` opens is read with
    /// (`XeTeX_default_input_mode`, `_encoding`; TeX and pdfTeX read bytes).
    pub(crate) fn default_input_mode(&mut self) -> (u8, u16) {
        if !self.unicode {
            return (0, 0);
        }
        let d = self.default_input();
        (
            u8::try_from(d & 0xFF).unwrap_or(0),
            u16::try_from(d >> 8).unwrap_or(0),
        )
    }

    /// §519: `name_of_file` from area, name and extension (quotes dropped,
    /// characters through `xchr`).
    pub(crate) fn pack_file_name(&mut self, n: i32, a: i32, e: i32) {
        let name = self.packed_name(
            self.str_bytes(ux(n)),
            self.str_bytes(ux(a)),
            self.str_bytes(ux(e)),
        );
        self.name_of_file = name;
    }

    /// §519 for a name kept as bytes (an `\openout` whatsit's).
    pub(crate) fn pack_file_name_bytes(&mut self, n: &[u8], a: &[u8], e: &[u8]) {
        self.name_of_file = self.packed_name(n, a, e);
    }

    fn packed_name(&self, n: &[u8], a: &[u8], e: &[u8]) -> Vec<u8> {
        [a, n, e]
            .iter()
            .flat_map(|s| s.iter())
            .filter(|&&c| c != b'"')
            .map(|&c| self.xchr[usize::from(c)])
            .collect()
    }

    /// `name_of_file` as the characters of a string: through `xord`, or
    /// (`XeTeX`'s `make_utf16_name`) decoded from UTF-8 into UTF-16 units.
    pub(crate) fn name_units(&self) -> alloc::vec::Vec<u32> {
        if !self.unicode {
            return self
                .name_of_file
                .iter()
                .map(|&b| u32::from(self.xord[usize::from(b)]))
                .collect();
        }
        let mut out = alloc::vec::Vec::new();
        let mut rest = &self.name_of_file[..];
        while !rest.is_empty() {
            let (c, n, _) = crate::input::decode_utf8_xetex(rest);
            if c > 0xFFFF {
                out.push(0xD800 + ((c - 0x1_0000) >> 10));
                out.push(0xDC00 + (c & 0x3FF));
            } else {
                out.push(c);
            }
            rest = &rest[n.min(rest.len())..];
        }
        out
    }

    /// §525: the string for `name_of_file`; also resets `cur_name`,
    /// `cur_area`, `cur_ext` to match it.
    pub(crate) fn make_name_string(&mut self) -> Result<i32, Jump> {
        let name_length = self.name_of_file.len();
        if self.pool_ptr + name_length > self.pool_size()
            || self.str_ptr == self.max_strings()
            || self.cur_length() > 0
        {
            return Ok(i32::from(b'?'));
        }
        let name = self.name_units();
        for &c in &name {
            self.append_char(c);
        }
        let result = i32::try_from(self.make_string()?).unwrap_or(0);
        let save_area_delimiter = self.area_delimiter;
        let save_ext_delimiter = self.ext_delimiter;
        let save_name_in_progress = self.name_in_progress;
        let save_stop_at_space = self.stop_at_space;
        self.name_in_progress = true;
        self.begin_name();
        self.stop_at_space = false;
        let mut k = 0;
        while k < name.len() && self.more_name(name[k])? {
            k += 1;
        }
        self.stop_at_space = save_stop_at_space;
        self.end_name()?;
        self.name_in_progress = save_name_in_progress;
        self.area_delimiter = save_area_delimiter;
        self.ext_delimiter = save_ext_delimiter;
        Ok(result)
    }

    /// web2c's `maketexstring`: a new string with these bytes.
    pub(crate) fn make_tex_string(&mut self, s: &[u8]) -> Result<i32, Jump> {
        self.str_room(s.len())?;
        for &c in s {
            self.append_char(c);
        }
        Ok(i32::try_from(self.make_string()?).unwrap_or(0))
    }

    /// §526 (web2c): scan a file name, or `{...}` with expansion.
    pub(crate) fn scan_file_name(&mut self) -> Result<(), Jump> {
        self.name_in_progress = true;
        let save_warning_index = self.warning_index;
        self.warning_index = self.cur_cs; // store `cur_cs` here to remember until later
        // Now we expand tokens and remove spaces from the input.
        self.get_nonblank_noncall()?;
        self.back_input()?; // return the last token to be read by either code path
        if self.cur_cmd == LEFT_BRACE {
            self.scan_file_name_braced()?;
        } else {
            self.name_in_progress = true;
            self.begin_name();
            self.get_nonblank_noncall()?;
            // (`biggest_usv` in `XeTeX`: a name goes on over any character)
            let biggest = if self.unicode { BIGGEST_USV } else { 255 };
            loop {
                if self.cur_cmd > OTHER_CHAR || self.cur_chr > biggest {
                    // not a character
                    self.back_input()?;
                    break;
                }
                // If `cur_chr` is a space and we're not scanning a token
                // list, check whether we're at the end of the buffer.
                if self.cur_chr == i32::from(b' ')
                    && self.state() != TOKEN_LIST
                    && self.cur_input.loc > self.cur_input.limit
                {
                    break;
                }
                if !self.more_name(crate::input::cu(self.cur_chr))? {
                    break;
                }
                self.get_x_token()?;
            }
        }
        self.end_name()?;
        self.name_in_progress = false;
        self.warning_index = save_warning_index; // restore `warning_index`
        Ok(())
    }

    /// web2c: `scan_file_name_braced`.
    fn scan_file_name_braced(&mut self) -> Result<(), Jump> {
        let save_scanner_status = self.scanner_status;
        let save_def_ref = core::mem::take(&mut self.def_ref);
        let save_cur_cs = self.cur_cs;
        self.cur_cs = self.warning_index; // for possible runaway error
        self.scan_toks(false, true)?;
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        let l = i32::try_from(self.pool_size() - self.pool_ptr).unwrap_or(i32::MAX);
        let text = core::mem::take(&mut self.def_ref);
        self.show_token_list(&text, crate::mem::NULL, l);
        self.set_selector(old_setting);
        let s = self.make_string()?;
        self.def_ref = save_def_ref; // (the list scanned is dropped)
        self.cur_cs = save_cur_cs;
        self.scanner_status = save_scanner_status;
        let save_stop_at_space = self.stop_at_space;
        self.stop_at_space = false; // allow spaces in file names
        self.begin_name();
        let chars: alloc::vec::Vec<u32> = if self.unicode {
            crate::strings::decode_units(&self.str_pool[self.str_start[s]..self.str_start[s + 1]])
                .collect()
        } else {
            self.str_pool[self.str_start[s]..self.str_start[s + 1]]
                .iter()
                .map(|&b| u32::from(b))
                .collect()
        };
        for c in chars {
            self.more_name(c)?;
        }
        self.stop_at_space = save_stop_at_space;
        Ok(())
    }

    /// §529: `s` is `".log"`, `".dvi"`, or `format_extension`.
    pub(crate) fn pack_job_name(&mut self, s: &[u8]) {
        self.cur_area = self.pool_str(b"");
        self.cur_ext = self.pool_str(s);
        self.cur_name = self.job_name();
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
    }

    /// §530: ask the user for another file name.
    pub(crate) fn prompt_file_name(&mut self, s: &[u8], e: &[u8]) -> Result<(), Jump> {
        if s == b"input file name" {
            self.print_err(b"I can't find file `");
        } else {
            self.print_err(b"I can't write on file `");
        }
        self.print_file_name(self.cur_name, self.cur_area, self.cur_ext);
        self.print_str(b"'.");
        if e == b".tex" || e.is_empty() {
            self.show_context();
        }
        self.print_ln();
        self.print_str(PROMPT_FILE_NAME_HELP_MSG);
        if !e.is_empty() {
            self.print_str(b"; default file extension is `");
            self.print_str(e);
            self.print_str(b"'");
        }
        self.print_str(b")");
        self.print_ln();
        self.print_nl(b"Please type another ");
        self.print_str(s);
        if self.interaction() < SCROLL_MODE {
            return self.fatal_error(b"*** (job aborted, file error in nonstop mode)");
        }
        let saved_cur_name = self.cur_name;
        let saved_cur_ext = self.cur_ext;
        let saved_cur_area = self.cur_area;
        self.prompt_input(b": ")?;
        // §531: scan file name in the buffer.
        self.begin_name();
        let mut k = self.first;
        while self.buffer[k] == u32::from(b' ') && k < self.last {
            k += 1;
        }
        loop {
            if k == self.last || !self.more_name(self.buffer[k])? {
                break;
            }
            k += 1;
        }
        self.end_name()?;
        let empty = self.pool_str(b"");
        if self.length(ux(self.cur_name)) == 0 && self.cur_ext == empty && self.cur_area == empty {
            self.cur_name = saved_cur_name;
            self.cur_ext = saved_cur_ext;
            self.cur_area = saved_cur_area;
        } else if self.cur_ext == empty {
            self.cur_ext = self.pool_str(e);
        }
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
        Ok(())
    }

    /// §534: open the transcript file and write the banner.
    pub(crate) fn open_log_file(&mut self) -> Result<(), Jump> {
        let old_setting = self.selector();
        if self.job_name() == 0 {
            let s = self.get_job_name(b"texput")?;
            self.set_job_name(s);
        }
        self.pack_job_name(b".log");
        loop {
            let name = self.name_of_file.clone();
            if let Some((id, printed)) = self.open_out(&name, FileKind::Other) {
                self.log_file.id = Some(id);
                self.out_wrote(crate::streams::LOG);
                self.name_of_file = printed;
                break;
            }
            // §535: try to get a different log file name.
            self.set_selector(TERM_ONLY);
            self.prompt_file_name(b"transcript file name", b".log")?;
        }
        let s = self.make_name_string()?;
        self.set_log_name(s);
        self.set_selector(LOG_ONLY);
        self.set_log_opened(true);
        // §536: print the banner line, including the date and time.
        let banner = self.banner();
        self.wlog_bytes(banner);
        let v = self.params.version_string;
        self.wlog_bytes(v);
        self.slow_print(self.format_ident);
        self.print_str(b"  ");
        // (the `sys_*` values, from the host: the job start does not change
        // within a job, and a machine tracks it there as a read, where it
        // leaves the `sys_*` fields out of the state it compares; with
        // values, a `Clock` read of the answer, DESIGN 7.17.12's `sys_*`)
        let now = self.host.now();
        self.clock_read(
            crate::track::Query::Now,
            &(now.year, now.month, now.day, now.minutes),
        );
        self.print_int(now.day);
        self.print_char(b' ');
        let months = b"JANFEBMARAPRMAYJUNJULAUGSEPOCTNOVDEC";
        let m = ux(now.month.clamp(1, 12));
        self.wlog_bytes(&months[3 * m - 3..3 * m]);
        self.print_char(b' ');
        self.print_int(now.year);
        self.print_char(b' ');
        self.print_two(now.minutes / 60);
        self.print_char(b':');
        self.print_two(now.minutes % 60);
        if self.etex_ex() {
            self.wlog_bytes(b"\nentering extended mode");
        }
        if self.params.shell_escape {
            self.wlog_bytes(b"\n ");
            if self.params.restricted_shell {
                self.wlog_bytes(b"restricted ");
            }
            self.wlog_bytes(b"\\write18 enabled.");
        }
        if self.params.file_line_error_style {
            self.wlog_bytes(b"\n file:line:error style messages enabled.");
        }
        if self.params.parse_first_line {
            self.wlog_bytes(b"\n %&-line parsing enabled.");
        }
        if let Some(t) = &self.params.translation {
            let name = t.name.clone();
            self.wlog_bytes(b"\n (");
            self.wlog_bytes(&name);
            self.wlog_bytes(b")");
        }
        if self.mltex_enabled_p {
            self.wlog_bytes(b"\nMLTeX v2.2 enabled");
        }
        self.input_stack[self.input_ptr] = self.cur_input.clone(); // make sure bottom level is in memory
        self.print_nl(b"**");
        let mut l = ux(self.input_stack[0].limit); // last position of first line
        if crate::input::ci(self.buffer[l]) == self.int_par(END_LINE_CHAR_CODE) {
            l = l.saturating_sub(1);
        }
        for k in 1..=l {
            self.print_chr(crate::input::ci(self.buffer[k]));
        }
        self.print_ln(); // now the transcript file contains the first line of input
        self.set_selector(old_setting + 2); // `log_only` or `term_and_log`
        Ok(())
    }

    /// web2c: `get_job_name`: `-jobname` overrides the given name.
    fn get_job_name(&mut self, name: &[u8]) -> Result<i32, Jump> {
        match self.params.job_name.clone() {
            Some(j) => self.make_tex_string(&j),
            None => self.make_tex_string(name),
        }
    }

    /// The file `\input` or `\openin` reads by `name`, and every other
    /// read of a file by name on TeX's path (`\pdffilesize`,
    /// `\pdfmdfivesum file`, `\pdffiledump`, `\pdfobj file`, an image):
    /// the host's, or, for a name the job stores, inside a rebuild, the
    /// store's value there (DESIGN 3.7: a load is served the store, never
    /// the file, which a step run again may have truncated), under the
    /// name the host finds it by. A load (7.17.5): by `lines` (`\input`,
    /// `\openin`), or whole.
    pub(crate) fn read_source(
        &mut self,
        name: &[u8],
        lines: bool,
    ) -> Option<crate::host::OpenedFile> {
        let found = self.host.read_file(name, FileKind::Tex);
        if !T::VALUES {
            return found;
        }
        let found = match self.tracker.stored(name) {
            // (named as the host finds it, or, not written yet, where the
            // job writes it: the output directory's)
            Some(v) => v.map(|contents| crate::host::OpenedFile {
                name: found.map_or_else(|| self.host.written_name(name), |f| f.name),
                contents,
            }),
            None => found,
        };
        let contents = found.as_ref().map(|f| &f.contents);
        if lines {
            self.tracker.load_lines(name, FileKind::Tex, contents);
        } else {
            self.tracker.load(name, FileKind::Tex, contents);
            if self.stop_after_load {
                self.load_stop = 1;
            }
        }
        found
    }

    /// §537: `\input` something.
    pub(crate) fn start_input(&mut self) -> Result<(), Jump> {
        self.scan_file_name()?; // set `cur_name` to desired file name
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
        let full_name;
        loop {
            self.begin_file_reading()?; // set up `cur_file` and new level of input
            // Kpathsea tries all the various ways to get the file.
            let name = self.name_of_file.clone();
            let found = self.read_source(&name, true);
            if let Some(f) = found {
                // web2c's `open_input`: drop a leading `./` the user did
                // not type.
                let found = f.name;
                self.name_of_file = if found.starts_with(b"./") && !name.starts_with(b"./") {
                    found[2..].to_vec()
                } else {
                    found.clone()
                };
                full_name = found;
                let found_name: alloc::sync::Arc<[u8]> = full_name.as_slice().into();
                if T::LINES {
                    self.tracker.lines_open(&f.contents);
                    if self.log_lines {
                        self.line_log.push((found_name.clone(), u32::MAX));
                    }
                }
                // (glyph origins name the file as it was asked for)
                self.origin_file_opened(&name, &full_name, &f.contents);
                let (mode, conv) = self.default_input_mode();
                self.input_file[self.in_open] = Some(AlphaFile {
                    data: f.contents,
                    name: found_name,
                    mode,
                    conv,
                    ..AlphaFile::default()
                });
                break;
            }
            self.end_file_reading(); // remove the level that didn't work
            self.prompt_file_name(b"input file name", b"")?;
        }
        self.cur_input.name = self.make_name_string()?;
        self.source_filename_stack[self.in_open] = self.cur_input.name;
        self.full_source_filename_stack[self.in_open] = self.make_tex_string(&full_name)?;
        if self.cur_input.name == i32::try_from(self.str_ptr).unwrap_or(0) - 1 {
            // we can try to conserve string pool space now
            let temp_str = self.search_string(self.cur_input.name);
            if temp_str > 0 {
                self.cur_input.name = temp_str;
                self.flush_string();
            }
        }
        if T::VALUES {
            let full = self.full_source_filename_stack[self.in_open];
            self.tracker
                .file(self.in_open, Some(self.str_bytes(ux(full))));
        }
        if self.job_name() == 0 {
            let s = self.get_job_name_str(self.cur_name)?;
            self.set_job_name(s);
            self.open_log_file()?;
        } // `open_log_file` doesn't `show_context`, so `limit` and `loc`
        // needn't be set to meaningful values yet
        let full = self.full_source_filename_stack[self.in_open];
        let len = i32::try_from(self.length(ux(full))).unwrap_or(0);
        self.print_sep(self.params.max_print_line - 2 - len);
        self.print_char(b'(');
        self.set_open_parens(self.open_parens() + 1);
        self.slow_print(full);
        self.update_terminal();
        let levels = self.int_par(TRACING_STACK_LEVELS_CODE);
        if levels > 0 {
            // web2c's `\tracingstacklevels` (`tracingstacklevels.ch`): the
            // file's input level
            self.begin_diagnostic();
            self.print_ln();
            self.print_char(b'~');
            let v = self.input_ptr - 1;
            if i32::try_from(v).is_ok_and(|v| v < levels) {
                for _ in 0..v {
                    self.print_char(b'.');
                }
            } else {
                self.print_char(b'~');
            }
            self.print_str(b"INPUT ");
            self.slow_print(self.cur_name);
            self.slow_print(self.cur_ext);
            self.print_ln();
            self.end_diagnostic(false);
        }
        self.cur_input.state = NEW_LINE;
        // (`SyncTeX`'s tag for the file: `synctex_start_input`)
        self.synctex_start_input(&full_name);
        // §538: read the first line of the new file.
        self.line = 1;
        let index = self.in_open;
        let mut f = self.input_file[index].take().unwrap_or_default();
        let from = f.pos;
        let r = self.input_ln(&mut f);
        match r {
            Ok(true) => self.start_line(&mut f, from, true),
            Ok(false) => self.end_of_file(&f, from),
            Err(_) => {}
        }
        self.input_file[index] = Some(f);
        r?;
        self.firm_up_the_line()?;
        if self.end_line_char_inactive() {
            self.cur_input.limit -= 1;
        } else {
            let c = self.int_par(END_LINE_CHAR_CODE);
            self.buffer[ux(self.cur_input.limit)] = crate::input::cu(c);
        }
        self.first = ux(self.cur_input.limit + 1);
        self.cur_input.loc = self.cur_input.start;
        // (a file level opened ends the open window at the next boundary:
        // DESIGN 4.3 item 1)
        self.window_event(crate::run::WindowEvent::FileOpened);
        Ok(())
    }

    /// web2c: `get_job_name(cur_name)`.
    fn get_job_name_str(&mut self, name: i32) -> Result<i32, Jump> {
        match self.params.job_name.clone() {
            Some(j) => self.make_tex_string(&j),
            None => Ok(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{engine, term_output};
    use crate::web::*;

    /// Oracle: `tex -ini '\input sub'`: the log starts with the banner,
    /// `**\input sub` and `(./sub.tex`; the terminal shows `(./sub.tex`.
    #[test]
    fn input_opens_the_log() {
        let mut t = engine();
        t.init_prim().unwrap();
        t.params.version_string = b" (TeX Live 2026/Arch Linux)";
        t.format_ident = t.pool_str(b" (INITEX)");
        t.fix_date_and_time();
        t.host
            .files
            .insert(b"sub.tex".to_vec(), b"x\\end\n".to_vec());
        // The command line, as §1337 leaves it.
        let line = b"\\input sub";
        for (d, &s) in t.buffer[1..=line.len()].iter_mut().zip(line) {
            *d = u32::from(s);
        }
        t.cur_input.state = NEW_LINE;
        t.cur_input.start = 1;
        t.cur_input.loc = 1;
        t.cur_input.limit = 11;
        t.buffer[11] = u32::from(b'\r');
        t.first = 12;
        let out = term_output(&mut t, |t| {
            t.get_x_token().unwrap();
        });
        assert_eq!(out, b"(./sub.tex");
        assert_eq!((t.cur_cmd, t.cur_chr), (LETTER, i32::from(b'x')));
        t.flush_log();
        let log = &t.host.written[&1];
        assert_eq!(
            core::str::from_utf8(log).unwrap(),
            "This is TeX, Version 3.141592653 (TeX Live 2026/Arch Linux) (INITEX)  \
             4 JUL 1776 12:00\n**\\input sub\n(./sub.tex"
        );
        assert_eq!(t.str_bytes(crate::input::ux(t.job_name)), b"sub");
    }
}
